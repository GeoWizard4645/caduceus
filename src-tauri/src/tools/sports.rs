//! Live scores — ESPN's public scoreboard JSON, no account required.
//!
//! # Why ESPN, and why it needs no key
//!
//! `site.api.espn.com/apis/site/v2/sports/...` is the same JSON ESPN's own
//! website and apps read to draw a scoreboard. It was not built as a public
//! API with a stable contract someone signed up for; it is simply reachable,
//! and every endpoint below was `curl`ed and read before anything here was
//! written against it, rather than coded to a guessed shape:
//!
//! * NFL — `sports/football/nfl/scoreboard`
//! * NBA — `sports/basketball/nba/scoreboard`
//! * MLB — `sports/baseball/mlb/scoreboard`
//! * F1 — `sports/racing/f1/scoreboard`
//! * World Cup / soccer — `sports/soccer/fifa.world/scoreboard`
//!
//! All five returned `200` with real data and no header or cookie beyond a
//! normal `User-Agent`. F1 and the World Cup were the two worth double
//! checking, and both did the interesting thing: F1's scoreboard is not a
//! final score at all but a set of session results (practice, qualifying,
//! race), which is why [`RaceWeekend`] exists as its own shape rather than
//! being forced into [`GameEvent`]; the World Cup scoreboard, tested against
//! `?dates=20260719` for the actual final, confirmed the same
//! competitors-with-scores shape as every other team sport ESPN covers.
//!
//! Like `markets.rs`, this module sends nothing about the person asking —
//! every request is a plain `GET` for public scores, with no key, account,
//! or identifying header of any kind.
//!
//! # No optional key
//!
//! ESPN's site API has no published key or higher-rate-limit tier to attach
//! — there is nothing to read from the keychain here, unlike `markets.rs`'s
//! optional CoinGecko header.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

const ESPN_BASE: &str = "https://site.api.espn.com/apis/site/v2/sports";

/// Applies to every request. Scores are worthless to a poller stuck waiting
/// past this.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Live scores are the fastest-moving data in this crate: a widget open
/// during a game wants something close to real time, but ESPN's own site
/// does not refresh faster than about this either — asking more often than
/// the source updates just spends the request on nothing.
const SCORES_TTL: Duration = Duration::from_secs(15);

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("Caduceus/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Could not start the request: {e}"))
}

fn describe_transport_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "ESPN did not answer in time.".into()
    } else if e.is_connect() {
        "Could not reach ESPN. Check that you are online.".into()
    } else {
        format!("ESPN's scoreboard could not be read: {e}")
    }
}

/// The leagues this module knows how to ask ESPN about. F1 and the World Cup
/// are called out by the product brief as priorities; NBA and MLB are along
/// for the ride because they are the same shape as NFL and cost nothing extra
/// to support once the parsing exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum League {
    Nfl,
    Nba,
    Mlb,
    F1,
    WorldCup,
}

impl League {
    fn path(self) -> &'static str {
        match self {
            League::Nfl => "football/nfl",
            League::Nba => "basketball/nba",
            League::Mlb => "baseball/mlb",
            League::F1 => "racing/f1",
            League::WorldCup => "soccer/fifa.world",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            League::Nfl => "NFL",
            League::Nba => "NBA",
            League::Mlb => "MLB",
            League::F1 => "Formula 1",
            League::WorldCup => "World Cup",
        }
    }

    fn is_racing(self) -> bool {
        matches!(self, League::F1)
    }
}

// ---------------------------------------------------------------------------
// Team sports (NFL, NBA, MLB, World Cup) — shared shape
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameStatus {
    /// "pre" | "in" | "post" — ESPN's own vocabulary, passed through as-is
    /// rather than reinvented, since the frontend needs exactly these three
    /// buckets to decide how to render a card.
    pub state: String,
    /// Human-readable, e.g. "Final", "Q3 4:12", "8/6 - 8:00 PM EDT".
    pub detail: String,
    pub completed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamScore {
    pub team: String,
    pub abbreviation: String,
    /// "home" | "away".
    pub home_away: String,
    /// ESPN sends the score as a string even though it is always numeric
    /// digits; kept as a string here rather than parsed, because a team that
    /// has not played yet gets `"0"` and a soccer match in extra time can
    /// carry a non-integer-looking detail ESPN itself decides how to format.
    pub score: String,
    pub winner: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameEvent {
    pub id: String,
    pub name: String,
    pub short_name: String,
    /// ISO 8601 UTC, straight from ESPN — formatting to a local time is a
    /// frontend concern, not something to bake into a cached payload.
    pub date: String,
    pub status: GameStatus,
    pub competitors: Vec<TeamScore>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scoreboard {
    pub league: League,
    pub events: Vec<GameEvent>,
    pub cached: bool,
}

#[derive(Debug, Deserialize)]
struct EspnScoreboardResponse {
    #[serde(default)]
    events: Vec<EspnEvent>,
}

#[derive(Debug, Deserialize)]
struct EspnEvent {
    id: String,
    name: String,
    #[serde(rename = "shortName")]
    short_name: String,
    date: String,
    status: EspnStatus,
    competitions: Vec<EspnCompetition>,
}

#[derive(Debug, Deserialize)]
struct EspnStatus {
    #[serde(rename = "type")]
    status_type: EspnStatusType,
}

#[derive(Debug, Deserialize)]
struct EspnStatusType {
    state: String,
    detail: String,
    completed: bool,
}

#[derive(Debug, Deserialize)]
struct EspnCompetition {
    #[serde(default)]
    competitors: Vec<EspnCompetitor>,
}

#[derive(Debug, Deserialize)]
struct EspnCompetitor {
    #[serde(rename = "homeAway")]
    home_away: String,
    #[serde(default)]
    score: Option<String>,
    #[serde(default)]
    winner: Option<bool>,
    #[serde(default)]
    team: Option<EspnTeam>,
}

#[derive(Debug, Deserialize)]
struct EspnTeam {
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(default)]
    abbreviation: String,
}

fn event_to_game(ev: EspnEvent) -> GameEvent {
    let competitors = ev
        .competitions
        .into_iter()
        .next()
        .map(|c| c.competitors)
        .unwrap_or_default()
        .into_iter()
        .map(|c| TeamScore {
            team: c.team.as_ref().map(|t| t.display_name.clone()).unwrap_or_default(),
            abbreviation: c.team.map(|t| t.abbreviation).unwrap_or_default(),
            home_away: c.home_away,
            score: c.score.unwrap_or_else(|| "0".into()),
            winner: c.winner,
        })
        .collect();

    GameEvent {
        id: ev.id,
        name: ev.name,
        short_name: ev.short_name,
        date: ev.date,
        status: GameStatus {
            state: ev.status.status_type.state,
            detail: ev.status.status_type.detail,
            completed: ev.status.status_type.completed,
        },
        competitors,
    }
}

// ---------------------------------------------------------------------------
// F1 — session-based, not team-vs-team
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverResult {
    pub position: u32,
    pub driver: String,
    pub winner: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaceSession {
    /// ESPN's own session abbreviation: "FP1", "FP2", "FP3", "Qual", "Race".
    pub session: String,
    pub completed: bool,
    /// Capped so one Grand Prix weekend (five sessions, ~20 drivers each)
    /// cannot balloon a cache entry; a widget wants the podium, not the
    /// full grid down to last place.
    pub top: Vec<DriverResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaceWeekend {
    pub id: String,
    /// e.g. "AWS Hungarian Grand Prix".
    pub name: String,
    pub date: String,
    pub status: GameStatus,
    pub sessions: Vec<RaceSession>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaceScoreboard {
    pub weekends: Vec<RaceWeekend>,
    pub cached: bool,
}

const F1_TOP_N: usize = 10;

#[derive(Debug, Deserialize)]
struct EspnRaceEvent {
    id: String,
    name: String,
    date: String,
    status: EspnStatus,
    #[serde(default)]
    competitions: Vec<EspnRaceSession>,
}

#[derive(Debug, Deserialize)]
struct EspnRaceSession {
    #[serde(rename = "type")]
    session_type: EspnSessionType,
    #[serde(default)]
    status: Option<EspnStatus>,
    #[serde(default)]
    competitors: Vec<EspnDriver>,
}

#[derive(Debug, Deserialize)]
struct EspnSessionType {
    abbreviation: String,
}

#[derive(Debug, Deserialize)]
struct EspnDriver {
    #[serde(default)]
    order: u32,
    #[serde(default)]
    winner: bool,
    #[serde(default)]
    athlete: Option<EspnAthlete>,
}

#[derive(Debug, Deserialize)]
struct EspnAthlete {
    #[serde(rename = "displayName")]
    display_name: String,
}

fn race_event_to_weekend(ev: EspnRaceEvent) -> RaceWeekend {
    let sessions = ev
        .competitions
        .into_iter()
        .map(|comp| {
            let mut drivers: Vec<DriverResult> = comp
                .competitors
                .into_iter()
                .map(|c| DriverResult {
                    position: c.order,
                    driver: c.athlete.map(|a| a.display_name).unwrap_or_default(),
                    winner: c.winner,
                })
                .collect();
            drivers.sort_by_key(|d| d.position);
            drivers.truncate(F1_TOP_N);
            RaceSession {
                session: comp.session_type.abbreviation,
                completed: comp.status.map(|s| s.status_type.completed).unwrap_or(false),
                top: drivers,
            }
        })
        .collect();

    RaceWeekend {
        id: ev.id,
        name: ev.name,
        date: ev.date,
        status: GameStatus {
            state: ev.status.status_type.state,
            detail: ev.status.status_type.detail,
            completed: ev.status.status_type.completed,
        },
        sessions,
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

enum CachedBoard {
    Team(Vec<GameEvent>),
    Race(Vec<RaceWeekend>),
}

pub struct SportsCache {
    inner: RwLock<HashMap<League, (u64, CachedBoard)>>,
}

impl Default for SportsCache {
    fn default() -> Self {
        Self { inner: RwLock::new(HashMap::new()) }
    }
}

impl SportsCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_team(&self, league: League) -> Option<Vec<GameEvent>> {
        let guard = self.inner.read();
        let (fetched_at, board) = guard.get(&league)?;
        if now_secs().saturating_sub(*fetched_at) > SCORES_TTL.as_secs() {
            return None;
        }
        match board {
            CachedBoard::Team(events) => Some(events.clone()),
            CachedBoard::Race(_) => None,
        }
    }

    fn get_race(&self, league: League) -> Option<Vec<RaceWeekend>> {
        let guard = self.inner.read();
        let (fetched_at, board) = guard.get(&league)?;
        if now_secs().saturating_sub(*fetched_at) > SCORES_TTL.as_secs() {
            return None;
        }
        match board {
            CachedBoard::Race(weekends) => Some(weekends.clone()),
            CachedBoard::Team(_) => None,
        }
    }

    fn put_team(&self, league: League, events: Vec<GameEvent>) {
        self.inner.write().insert(league, (now_secs(), CachedBoard::Team(events)));
    }

    fn put_race(&self, league: League, weekends: Vec<RaceWeekend>) {
        self.inner.write().insert(league, (now_secs(), CachedBoard::Race(weekends)));
    }
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

async fn get_scoreboard_json(league: League) -> Result<String, String> {
    let url = format!("{ESPN_BASE}/{}/scoreboard", league.path());
    let response = client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| describe_transport_error(&e))?;

    if !response.status().is_success() {
        return Err(format!("ESPN returned {} for {}.", response.status().as_u16(), league.display_name()));
    }

    response.text().await.map_err(|e| describe_transport_error(&e))
}

/// Fetch (or reuse) the current scoreboard for a team sport (NFL, NBA, MLB,
/// or the World Cup). Use [`fetch_f1`] for F1, whose scoreboard has a
/// different shape.
///
/// Calling this with [`League::F1`] returns an error rather than a wrong
/// answer — F1 events have no `competitors`-with-`score` to parse, and
/// silently returning an empty scoreboard would look like "no races" instead
/// of "wrong function".
pub async fn fetch_scoreboard(cache: &SportsCache, league: League) -> Result<Scoreboard, String> {
    if league.is_racing() {
        return Err("F1 is a session-based sport; call fetch_f1 instead of fetch_scoreboard.".into());
    }

    if let Some(events) = cache.get_team(league) {
        return Ok(Scoreboard { league, events, cached: true });
    }

    let body = get_scoreboard_json(league).await?;
    let parsed: EspnScoreboardResponse = serde_json::from_str(&body)
        .map_err(|_| format!("ESPN sent something unreadable for {}.", league.display_name()))?;

    let events: Vec<GameEvent> = parsed.events.into_iter().map(event_to_game).collect();
    cache.put_team(league, events.clone());
    Ok(Scoreboard { league, events, cached: false })
}

/// Fetch (or reuse) the current Formula 1 weekend(s): practice, qualifying,
/// and race sessions, each with its top finishers.
pub async fn fetch_f1(cache: &SportsCache) -> Result<RaceScoreboard, String> {
    if let Some(weekends) = cache.get_race(League::F1) {
        return Ok(RaceScoreboard { weekends, cached: true });
    }

    let body = get_scoreboard_json(League::F1).await?;
    let parsed: EspnScoreboardResponseRaw = serde_json::from_str(&body)
        .map_err(|_| "ESPN sent something unreadable for Formula 1.".to_string())?;

    let weekends: Vec<RaceWeekend> = parsed.events.into_iter().map(race_event_to_weekend).collect();
    cache.put_race(League::F1, weekends.clone());
    Ok(RaceScoreboard { weekends, cached: false })
}

#[derive(Debug, Deserialize)]
struct EspnScoreboardResponseRaw {
    #[serde(default)]
    events: Vec<EspnRaceEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Recorded payloads -------------------------------------------------
    //
    // Trimmed real bodies from `curl`ing each ESPN scoreboard endpoint while
    // writing this module. No test here touches the network.

    const NFL_SAMPLE: &str = r#"{"events":[{"id":"401873271","name":"Carolina Panthers at Arizona Cardinals","shortName":"CAR VS ARI","date":"2026-08-07T00:00Z","status":{"type":{"state":"pre","detail":"Thu, August 6th at 8:00 PM EDT","completed":false}},"competitions":[{"competitors":[{"homeAway":"home","score":"0","winner":null,"team":{"displayName":"Arizona Cardinals","abbreviation":"ARI"}},{"homeAway":"away","score":"0","winner":null,"team":{"displayName":"Carolina Panthers","abbreviation":"CAR"}}]}]}]}"#;

    const MLB_SAMPLE: &str = r#"{"events":[{"id":"401696668","name":"Seattle Mariners at Texas Rangers","shortName":"SEA @ TEX","date":"2026-07-27T23:05Z","status":{"type":{"state":"post","detail":"Final","completed":true}},"competitions":[{"competitors":[{"homeAway":"home","score":"7","winner":true,"team":{"displayName":"Texas Rangers","abbreviation":"TEX"}},{"homeAway":"away","score":"3","winner":false,"team":{"displayName":"Seattle Mariners","abbreviation":"SEA"}}]}]}]}"#;

    const WORLD_CUP_SAMPLE: &str = r#"{"events":[{"id":"731611","name":"Argentina at Spain","shortName":"ARG @ ESP","date":"2026-07-19T19:00Z","status":{"type":{"state":"post","detail":"Final Score - After Extra Time","completed":true}},"competitions":[{"competitors":[{"homeAway":"home","score":"1","winner":true,"team":{"displayName":"Spain","abbreviation":"ESP"}},{"homeAway":"away","score":"0","winner":false,"team":{"displayName":"Argentina","abbreviation":"ARG"}}]}]}]}"#;

    const F1_SAMPLE: &str = r#"{"events":[{"id":"600057440","name":"AWS Hungarian Grand Prix","date":"2026-07-24T11:30Z","status":{"type":{"state":"post","detail":"Final","completed":true}},"competitions":[{"type":{"abbreviation":"FP1"},"status":{"type":{"state":"post","detail":"Final","completed":true}},"competitors":[{"order":1,"winner":false,"athlete":{"displayName":"Charles Leclerc"}},{"order":2,"winner":false,"athlete":{"displayName":"Max Verstappen"}}]},{"type":{"abbreviation":"Race"},"status":{"type":{"state":"post","detail":"Final","completed":true}},"competitors":[{"order":1,"winner":true,"athlete":{"displayName":"Lando Norris"}},{"order":2,"winner":false,"athlete":{"displayName":"Max Verstappen"}},{"order":3,"winner":false,"athlete":{"displayName":"Kimi Antonelli"}}]}]}]}"#;

    // ---- League routing ------------------------------------------------

    #[test]
    fn each_league_maps_to_the_espn_path_confirmed_working() {
        assert_eq!(League::Nfl.path(), "football/nfl");
        assert_eq!(League::Nba.path(), "basketball/nba");
        assert_eq!(League::Mlb.path(), "baseball/mlb");
        assert_eq!(League::F1.path(), "racing/f1");
        assert_eq!(League::WorldCup.path(), "soccer/fifa.world");
    }

    // ---- Team sports -----------------------------------------------------

    #[test]
    fn nfl_payload_parses_a_scheduled_game_with_zero_scores() {
        let parsed: EspnScoreboardResponse = serde_json::from_str(NFL_SAMPLE).unwrap();
        let games: Vec<GameEvent> = parsed.events.into_iter().map(event_to_game).collect();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].status.state, "pre");
        assert!(!games[0].status.completed);
        assert_eq!(games[0].competitors[0].score, "0");
    }

    #[test]
    fn mlb_payload_parses_a_finished_game_with_a_winner() {
        let parsed: EspnScoreboardResponse = serde_json::from_str(MLB_SAMPLE).unwrap();
        let games: Vec<GameEvent> = parsed.events.into_iter().map(event_to_game).collect();
        assert!(games[0].status.completed);
        let home = games[0].competitors.iter().find(|c| c.home_away == "home").unwrap();
        assert_eq!(home.score, "7");
        assert_eq!(home.winner, Some(true));
    }

    #[test]
    fn world_cup_payload_uses_the_same_shape_as_every_other_team_sport() {
        let parsed: EspnScoreboardResponse = serde_json::from_str(WORLD_CUP_SAMPLE).unwrap();
        let games: Vec<GameEvent> = parsed.events.into_iter().map(event_to_game).collect();
        assert_eq!(games[0].status.detail, "Final Score - After Extra Time");
        let winner = games[0].competitors.iter().find(|c| c.winner == Some(true)).unwrap();
        assert_eq!(winner.team, "Spain");
    }

    // ---- F1 ----------------------------------------------------------------

    #[test]
    fn f1_payload_parses_multiple_sessions_per_weekend() {
        let parsed: EspnScoreboardResponseRaw = serde_json::from_str(F1_SAMPLE).unwrap();
        let weekends: Vec<RaceWeekend> = parsed.events.into_iter().map(race_event_to_weekend).collect();
        assert_eq!(weekends.len(), 1);
        assert_eq!(weekends[0].sessions.len(), 2);
        assert_eq!(weekends[0].sessions[0].session, "FP1");
        assert_eq!(weekends[0].sessions[1].session, "Race");
    }

    #[test]
    fn f1_race_winner_is_the_driver_marked_winner_at_position_one() {
        let parsed: EspnScoreboardResponseRaw = serde_json::from_str(F1_SAMPLE).unwrap();
        let weekends: Vec<RaceWeekend> = parsed.events.into_iter().map(race_event_to_weekend).collect();
        let race = weekends[0].sessions.iter().find(|s| s.session == "Race").unwrap();
        assert_eq!(race.top[0].driver, "Lando Norris");
        assert!(race.top[0].winner);
        assert!(!race.top[1].winner);
    }

    #[test]
    fn f1_drivers_are_sorted_and_capped_at_the_top_n() {
        let mut competitors = Vec::new();
        for i in 1..=25u32 {
            competitors.push(EspnDriver { order: 26 - i, winner: false, athlete: Some(EspnAthlete { display_name: format!("Driver {i}") }) });
        }
        let session = EspnRaceSession {
            session_type: EspnSessionType { abbreviation: "Race".into() },
            status: None,
            competitors,
        };
        let event = EspnRaceEvent {
            id: "1".into(),
            name: "Test GP".into(),
            date: "2026-01-01T00:00Z".into(),
            status: EspnStatus { status_type: EspnStatusType { state: "post".into(), detail: "Final".into(), completed: true } },
            competitions: vec![session],
        };
        let weekend = race_event_to_weekend(event);
        assert_eq!(weekend.sessions[0].top.len(), F1_TOP_N);
        assert_eq!(weekend.sessions[0].top[0].position, 1, "must be sorted by finishing position, not input order");
    }

    #[test]
    fn calling_fetch_scoreboard_shape_on_f1_is_rejected_by_type_not_by_luck() {
        // fetch_scoreboard's F1 guard is exercised via an async runtime in
        // integration; here we assert the routing predicate it relies on.
        assert!(League::F1.is_racing());
        assert!(!League::Nfl.is_racing());
    }

    // ---- Cache ---------------------------------------------------------

    #[test]
    fn a_fresh_team_entry_is_reused_and_marked_cached() {
        let cache = SportsCache::new();
        let parsed: EspnScoreboardResponse = serde_json::from_str(MLB_SAMPLE).unwrap();
        let games: Vec<GameEvent> = parsed.events.into_iter().map(event_to_game).collect();
        cache.put_team(League::Mlb, games);
        assert!(cache.get_team(League::Mlb).is_some());
    }

    #[test]
    fn a_stale_team_entry_is_not_reused() {
        let cache = SportsCache::new();
        let old = now_secs() - SCORES_TTL.as_secs() - 1;
        cache.inner.write().insert(League::Nfl, (old, CachedBoard::Team(Vec::new())));
        assert!(cache.get_team(League::Nfl).is_none());
    }

    #[test]
    fn leagues_do_not_share_a_cache_slot() {
        let cache = SportsCache::new();
        let parsed: EspnScoreboardResponse = serde_json::from_str(NFL_SAMPLE).unwrap();
        let games: Vec<GameEvent> = parsed.events.into_iter().map(event_to_game).collect();
        cache.put_team(League::Nfl, games);
        assert!(cache.get_team(League::Mlb).is_none(), "MLB must not see NFL's cached board");
    }

    #[test]
    fn a_team_cache_slot_is_not_returned_as_a_race_board_or_vice_versa() {
        let cache = SportsCache::new();
        let parsed: EspnScoreboardResponse = serde_json::from_str(NFL_SAMPLE).unwrap();
        let games: Vec<GameEvent> = parsed.events.into_iter().map(event_to_game).collect();
        cache.put_team(League::Nfl, games);
        assert!(cache.get_race(League::Nfl).is_none());
    }
}
