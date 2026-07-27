//! Citations for the page you are looking at.
//!
//! Reads the frontmost browser's current tab — title, URL, and whatever the
//! page says about its author and date — and formats it in the styles people
//! are actually asked for.
//!
//! # Where the metadata comes from
//!
//! The tab's title and URL come from the browser over AppleScript, which needs
//! Automation permission and nothing else. Author and date come from the page's
//! own `<meta>` tags, fetched over HTTP — and *only* when the user asks for it,
//! because an app that quietly fetches every page you visit is not a citation
//! tool, it is a tracker with a bibliography.
//!
//! When the metadata is missing, the citation says so with `n.d.` and the site
//! name rather than inventing an author. A citation with a plausible-looking
//! made-up author is worse than an obviously incomplete one: the second gets
//! fixed, the first gets handed in.

use serde::{Deserialize, Serialize};

use super::run;

/// The citation styles offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Style {
    Mla9,
    Apa7,
    Chicago,
    Harvard,
    Ieee,
    Vancouver,
    Bibtex,
}

impl Style {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mla9 => "MLA 9th",
            Self::Apa7 => "APA 7th",
            Self::Chicago => "Chicago (notes)",
            Self::Harvard => "Harvard",
            Self::Ieee => "IEEE",
            Self::Vancouver => "Vancouver",
            Self::Bibtex => "BibTeX",
        }
    }

    pub const ALL: &'static [Style] = &[
        Style::Mla9,
        Style::Apa7,
        Style::Chicago,
        Style::Harvard,
        Style::Ieee,
        Style::Vancouver,
        Style::Bibtex,
    ];
}

/// What we know about the page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub site: Option<String>,
    /// `YYYY-MM-DD` or `YYYY` — whatever the page actually said.
    pub published: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub style: Style,
    pub label: String,
    pub text: String,
}

// ---------------------------------------------------------------------------
// Reading the browser
// ---------------------------------------------------------------------------

/// The frontmost browser tab, if a supported browser is in front.
///
/// Chromium browsers and Safari have different AppleScript vocabularies, so
/// each is asked in its own dialect. Browsers that are not running are skipped
/// without being launched — asking about a browser should never open one.
pub fn current_page() -> Result<Source, String> {
    for (app, script) in [
        ("Google Chrome", CHROMIUM),
        ("Arc", CHROMIUM),
        ("Brave Browser", CHROMIUM),
        ("Microsoft Edge", CHROMIUM),
        ("Vivaldi", CHROMIUM),
        ("Safari", SAFARI),
    ] {
        if !is_running(app) {
            continue;
        }
        let filled = script.replace("{{app}}", app);
        let Ok(output) = run("osascript", &["-e", &filled]) else { continue };
        let mut parts = output.splitn(2, '\u{1}');
        let (Some(title), Some(url)) = (parts.next(), parts.next()) else { continue };
        if url.trim().is_empty() {
            continue;
        }
        return Ok(Source {
            title: title.trim().to_string(),
            url: url.trim().to_string(),
            site: site_from(url),
            ..Default::default()
        });
    }

    Err("No supported browser has a page open in front. Chrome, Arc, Brave, Edge, Vivaldi \
         and Safari all work."
        .into())
}

fn is_running(app: &str) -> bool {
    run(
        "osascript",
        &["-e", &format!("application \"{app}\" is running")],
    )
    .map(|out| out.trim() == "true")
    .unwrap_or(false)
}

// U+0001 as the separator: it cannot occur in a title or a URL, unlike every
// punctuation character somebody has put in a headline.
const CHROMIUM: &str = r#"tell application "{{app}}"
    set t to title of active tab of front window
    set u to URL of active tab of front window
    return t & (ASCII character 1) & u
end tell"#;

const SAFARI: &str = r#"tell application "Safari"
    set t to name of current tab of front window
    set u to URL of current tab of front window
    return t & (ASCII character 1) & u
end tell"#;

/// `www.bbc.co.uk/news/…` → `bbc.co.uk`.
fn site_from(url: &str) -> Option<String> {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let host = without_scheme.split('/').next()?;
    Some(host.strip_prefix("www.").unwrap_or(host).to_string())
}

// ---------------------------------------------------------------------------
// Enriching from the page
// ---------------------------------------------------------------------------

/// Fill in author and date from the page's own `<meta>` tags.
///
/// Best effort and explicitly optional. A failure here leaves the citation
/// usable rather than blocking it — an incomplete citation the user can fix
/// beats an error message they cannot.
pub async fn enrich(mut source: Source) -> Source {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        // Some sites serve a very different page to something that does not
        // look like a browser, including no metadata at all.
        .user_agent("Mozilla/5.0 (Macintosh) Caduceus/1.0")
        .build()
    else {
        return source;
    };

    let Ok(response) = client.get(&source.url).send().await else { return source };
    let Ok(body) = response.text().await else { return source };
    // The head is where metadata lives; a news page can be a megabyte of
    // comments after it.
    let head = &body[..body.len().min(200_000)];

    source.author = source.author.or_else(|| {
        first_meta(
            head,
            &["citation_author", "author", "article:author", "og:article:author", "twitter:creator"],
        )
    });
    source.published = source.published.or_else(|| {
        first_meta(
            head,
            &[
                "citation_publication_date",
                "article:published_time",
                "datePublished",
                "og:article:published_time",
                "date",
            ],
        )
        .map(|d| d.chars().take(10).collect())
    });
    source.site = source.site.or_else(|| first_meta(head, &["og:site_name", "application-name"]));

    if source.title.is_empty() {
        if let Some(title) = first_meta(head, &["og:title", "citation_title"]) {
            source.title = title;
        }
    }

    source
}

/// The first of these `<meta>` names or properties that the page defines.
///
/// A deliberately small regex rather than an HTML parser: this reads five
/// attributes out of a `<head>` and has no business pulling a parser — and a
/// tag it fails to understand degrades to "no author", which is handled.
fn first_meta(html: &str, keys: &[&str]) -> Option<String> {
    for key in keys {
        for tag in html.split("<meta").skip(1) {
            let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
            let lower = tag.to_lowercase();
            let matches_key = [
                format!("name=\"{key}\""),
                format!("name='{key}'"),
                format!("property=\"{key}\""),
                format!("property='{key}'"),
                format!("itemprop=\"{key}\""),
            ]
            .iter()
            .any(|needle| lower.contains(&needle.to_lowercase()));

            if !matches_key {
                continue;
            }
            if let Some(content) = attribute(tag, "content") {
                let cleaned = decode_entities(content.trim());
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
    }
    None
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_lowercase();
    let at = lower.find(&format!("{name}="))?;
    let rest = &tag[at + name.len() + 1..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = rest[1..].find(quote)? + 1;
    Some(&rest[1..end])
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// Format one source in every style, so the user picks rather than re-runs.
pub fn format_all(source: &Source, accessed: &str) -> Vec<Citation> {
    Style::ALL
        .iter()
        .map(|style| Citation {
            style: *style,
            label: style.label().into(),
            text: format_one(source, *style, accessed),
        })
        .collect()
}

pub fn format_one(source: &Source, style: Style, accessed: &str) -> String {
    let title = source.title.trim();
    let site = source.site.clone().unwrap_or_else(|| site_from(&source.url).unwrap_or_default());
    let url = &source.url;
    let year = source
        .published
        .as_ref()
        .and_then(|d| d.get(..4).map(str::to_string))
        .unwrap_or_else(|| "n.d.".into());
    let full_date = source.published.clone().unwrap_or_else(|| "n.d.".into());

    match style {
        Style::Mla9 => {
            let author = source
                .author
                .as_ref()
                .map(|a| format!("{}. ", surname_first(a)))
                .unwrap_or_default();
            format!("{author}\"{title}.\" {site}, {full_date}, {url}. Accessed {accessed}.")
        }
        Style::Apa7 => {
            let author = source
                .author
                .as_ref()
                .map(|a| format!("{}. ", initialised(a)))
                .unwrap_or_else(|| format!("{site}. "));
            format!("{author}({year}). {title}. {site}. {url}")
        }
        Style::Chicago => {
            let author = source.author.as_ref().map(|a| format!("{a}. ")).unwrap_or_default();
            format!("{author}\"{title}.\" {site}. Last modified {full_date}. {url}.")
        }
        Style::Harvard => {
            let author = source
                .author
                .clone()
                .unwrap_or_else(|| site.clone());
            format!("{author} ({year}) '{title}', {site}. Available at: {url} (Accessed: {accessed}).")
        }
        Style::Ieee => {
            let author = source
                .author
                .as_ref()
                .map(|a| format!("{}, ", initials_first(a)))
                .unwrap_or_default();
            format!("{author}\"{title},\" {site}, {year}. [Online]. Available: {url}. [Accessed: {accessed}].")
        }
        Style::Vancouver => {
            let author = source.author.as_ref().map(|a| format!("{a}. ")).unwrap_or_default();
            format!("{author}{title} [Internet]. {site}; {year} [cited {accessed}]. Available from: {url}")
        }
        Style::Bibtex => {
            let key = bibtex_key(source, &year);
            let author = source.author.clone().unwrap_or_else(|| site.clone());
            format!(
                "@misc{{{key},\n  title        = {{{title}}},\n  author       = {{{author}}},\n  \
                 howpublished = {{\\url{{{url}}}}},\n  year         = {{{year}}},\n  \
                 note         = {{Accessed: {accessed}}}\n}}"
            )
        }
    }
}

/// `Jane Smith` → `Smith, Jane`. Anything that is not two-plus words is left alone.
fn surname_first(name: &str) -> String {
    let parts: Vec<_> = name.split_whitespace().collect();
    if parts.len() < 2 {
        return name.to_string();
    }
    let (last, rest) = parts.split_last().unwrap();
    format!("{last}, {}", rest.join(" "))
}

/// `Jane Ann Smith` → `Smith, J. A.` — APA's shape.
fn initialised(name: &str) -> String {
    let parts: Vec<_> = name.split_whitespace().collect();
    if parts.len() < 2 {
        return name.to_string();
    }
    let (last, rest) = parts.split_last().unwrap();
    let initials: Vec<String> = rest
        .iter()
        .filter_map(|p| p.chars().next())
        .map(|c| format!("{}.", c.to_uppercase()))
        .collect();
    format!("{last}, {}", initials.join(" "))
}

/// `Jane Ann Smith` → `J. A. Smith` — IEEE's shape.
fn initials_first(name: &str) -> String {
    let parts: Vec<_> = name.split_whitespace().collect();
    if parts.len() < 2 {
        return name.to_string();
    }
    let (last, rest) = parts.split_last().unwrap();
    let initials: Vec<String> = rest
        .iter()
        .filter_map(|p| p.chars().next())
        .map(|c| format!("{}.", c.to_uppercase()))
        .collect();
    format!("{} {last}", initials.join(" "))
}

fn bibtex_key(source: &Source, year: &str) -> String {
    let who = source
        .author
        .as_ref()
        .and_then(|a| a.split_whitespace().last().map(str::to_string))
        .or_else(|| source.site.clone())
        .unwrap_or_else(|| "web".into());
    let word = source
        .title
        .split_whitespace()
        .find(|w| w.len() > 3)
        .unwrap_or("source");
    let clean = |s: &str| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    };
    format!("{}{}{}", clean(&who), year.chars().filter(char::is_ascii_digit).collect::<String>(), clean(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        Source {
            title: "How Sleep Works".into(),
            url: "https://www.example.com/sleep".into(),
            author: Some("Jane Ann Smith".into()),
            site: Some("Example".into()),
            published: Some("2025-03-14".into()),
        }
    }

    #[test]
    fn each_style_puts_the_name_in_its_own_shape() {
        assert_eq!(surname_first("Jane Ann Smith"), "Smith, Jane Ann");
        assert_eq!(initialised("Jane Ann Smith"), "Smith, J. A.");
        assert_eq!(initials_first("Jane Ann Smith"), "J. A. Smith");
        // A single word is a handle or an organisation; reformatting it is wrong.
        assert_eq!(surname_first("Reuters"), "Reuters");
        assert_eq!(initialised("Reuters"), "Reuters");
    }

    #[test]
    fn every_style_produces_something_containing_the_url_and_title() {
        for style in Style::ALL {
            let text = format_one(&source(), *style, "27 July 2026");
            assert!(text.contains("How Sleep Works"), "{style:?} lost the title");
            assert!(text.contains("https://www.example.com/sleep"), "{style:?} lost the URL");
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn a_missing_author_and_date_are_admitted_rather_than_invented() {
        let bare = Source {
            title: "Untitled".into(),
            url: "https://example.com/x".into(),
            ..Default::default()
        };
        let apa = format_one(&bare, Style::Apa7, "27 July 2026");
        // "n.d." is the standard way to say the date is unknown. What must not
        // happen is a year appearing from nowhere.
        assert!(apa.contains("n.d."), "got {apa}");
        assert!(!apa.contains("2026)"), "invented a publication year: {apa}");
    }

    #[test]
    fn the_site_is_derived_from_the_url_when_the_page_does_not_say() {
        assert_eq!(site_from("https://www.bbc.co.uk/news/x"), Some("bbc.co.uk".into()));
        assert_eq!(site_from("https://example.com"), Some("example.com".into()));
    }

    #[test]
    fn metadata_is_read_out_of_the_head() {
        let html = r#"<head>
            <meta property="og:site_name" content="The Example">
            <meta name="author" content="Jane Smith">
            <meta property="article:published_time" content="2025-03-14T10:00:00Z">
        </head>"#;
        assert_eq!(first_meta(html, &["author"]).as_deref(), Some("Jane Smith"));
        assert_eq!(first_meta(html, &["og:site_name"]).as_deref(), Some("The Example"));
        assert!(first_meta(html, &["nothing-like-this"]).is_none());
    }

    #[test]
    fn entities_in_metadata_are_decoded() {
        let html = r#"<meta name="author" content="Smith &amp; Jones">"#;
        assert_eq!(first_meta(html, &["author"]).as_deref(), Some("Smith & Jones"));
    }

    #[test]
    fn a_bibtex_key_is_stable_and_safe_to_paste() {
        let key = bibtex_key(&source(), "2025");
        assert_eq!(key, "smith2025sleep");
        // No spaces, braces or punctuation — those break the .bib file.
        assert!(key.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
