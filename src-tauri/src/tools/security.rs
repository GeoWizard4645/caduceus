//! Privacy & security utilities (roadmap items 116-125).
//!
//! Everything here either shells out to something macOS already ships
//! (`defaults`, `osascript`, `/usr/bin/log`, `socketfilterfw`, `open`) or uses
//! crates already in the dependency tree. Nothing new was added to
//! `Cargo.toml` — see the file-vault section below for what that constraint
//! costs.
//!
//! # Already built elsewhere — not duplicated here
//!
//! * **Password generator** — `tools::dev::ToolId::Password` already exists
//!   (24-character CSPRNG password, drawn from the same ambiguity-free
//!   alphabet this file uses). Rather than ship a second "generate a random
//!   password" button, this file adds the thing dev.rs does *not* have: a
//!   diceware-style **passphrase** generator (multiple real words, easy to
//!   read off a screen and type on a phone, comparable entropy to a shorter
//!   random string). See "1. Passphrase generator" below.
//! * **Hidden Finder files toggle** — `tools::system::SystemAction::ToggleHiddenFiles`
//!   already does exactly roadmap item 118 (`defaults write
//!   com.apple.finder AppleShowAllFiles`, then `killall Finder`). Building a
//!   second copy here would just be a second place for that logic to drift
//!   out of sync with the first, so it is deliberately skipped. If a security
//!   agent needs it in this module's namespace, wire a call to
//!   `system::run(SystemAction::ToggleHiddenFiles)` rather than reimplementing it.
//! * **Speaker mute** — `tools::system::SystemAction::ToggleMute` toggles
//!   *output* (speaker) mute via `output muted of (get volume settings)`.
//!   That AppleScript property has no microphone equivalent — macOS simply
//!   does not expose an "input muted" boolean — which is why item 119
//!   (microphone mute) below has to fake it by remembering the input volume
//!   and zeroing it, rather than flipping a bool.
//!
//! # Documented gaps — not half-built
//!
//! * **TouchID app lock** (item 122) — see [`touch_id_available`]. Local
//!   Authentication (the framework behind a TouchID prompt) has no binding
//!   crate anywhere in this dependency tree, not even transitively —
//!   `Cargo.lock` has no `objc2-local-authentication` and none of the
//!   `objc2-*` crates already vendored (app-kit, foundation, core-*, ...)
//!   cover it. Reaching it would mean either a new dependency (out of scope —
//!   "do not run cargo add") or hand-writing raw `objc2::extern_class!`
//!   bindings to a framework this crate has never touched, which is exactly
//!   the kind of unverified surface that does not belong in a single-file,
//!   no-new-dependency pass. Flagged as a real gap rather than shipping a
//!   toggle that does not actually authenticate anyone.
//!
//! # Wrappers another agent needs to register
//!
//! This file cannot touch `commands.rs`, so nothing here is reachable from
//! the frontend yet. What is missing, in the style already used for
//! `system::run` / `media::run`:
//!
//! ```text
//! #[tauri::command]
//! pub fn security_generate_passphrase(words: usize) -> ToolOutcome { .. }
//! #[tauri::command]
//! pub fn security_clipboard_auto_clear(seconds: u64) -> ToolOutcome { .. }
//! #[tauri::command]
//! pub fn security_cancel_auto_clear() -> ToolOutcome { .. }
//! #[tauri::command]
//! pub fn security_mic_muted() -> Result<bool, String> { .. }
//! #[tauri::command]
//! pub fn security_set_mic_muted(mute: bool) -> ToolOutcome { .. }
//! #[tauri::command]
//! pub async fn security_activity_log(minutes: u32) -> Result<Vec<ActivityEvent>, String> { .. }
//! #[tauri::command]
//! pub fn security_firewall_state() -> Result<FirewallState, String> { .. }
//! #[tauri::command]
//! pub fn security_open_firewall_settings() -> ToolOutcome { .. }
//! #[tauri::command]
//! pub fn security_lock_file(path: String, passphrase: String, delete_original: bool) -> ToolOutcome { .. }
//! #[tauri::command]
//! pub fn security_unlock_file(path: String, passphrase: String) -> ToolOutcome { .. }
//! ```
//!
//! `security_activity_log` and anything shelling to `/usr/bin/log` should go
//! through `blocking_outcome`/`spawn_blocking` the same way `system::machine_summary`
//! does — a `log show` over an hour of TCC noise is not instant.
//!
//! # A note on "reversible and must say exactly what it changed"
//!
//! Every function that touches system state here (mic volume, the clipboard,
//! files on disk) either reports the exact prior value it is restoring, or —
//! for the file vault and the firewall — refuses to make the risky part of
//! the change itself (overwriting an existing destination file; flipping a
//! setting that needs an admin password) and says so.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use regex::Regex;
use serde::Serialize;

use super::{output_with_timeout, ToolOutcome, TOOL_TIMEOUT};
use crate::clipboard::crypto;

// ---------------------------------------------------------------------------
// Shared randomness (CSPRNG, unbiased selection)
// ---------------------------------------------------------------------------
//
// Deliberately not imported from `tools::dev` — its `random_bytes`/`random_index`
// are private to that file, and duplicating twelve lines of rejection sampling
// here is a smaller cost than making them `pub(crate)` in a file this agent
// does not own. The algorithm is identical to dev.rs's on purpose: getrandom's
// CSPRNG, rejection sampling to avoid modulo bias.

fn random_bytes(buffer: &mut [u8]) -> bool {
    getrandom::fill(buffer).is_ok()
}

/// A uniformly distributed index into `len` items, without modulo bias.
fn random_index(len: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    let limit = u32::MAX - (u32::MAX % len as u32) - 1;
    loop {
        let mut raw = [0u8; 4];
        if !random_bytes(&mut raw) {
            return 0;
        }
        let value = u32::from_le_bytes(raw);
        if value <= limit {
            return (value % len as u32) as usize;
        }
    }
}

// ---------------------------------------------------------------------------
// 1. Passphrase generator (item 116, the memorable half of it)
// ---------------------------------------------------------------------------
//
// Diceware-style: pick N words independently and uniformly at random from a
// fixed list, join them with a separator. Entropy is `N * log2(list_len)`
// bits, computed from the list's *actual* length rather than a hardcoded
// constant, so the number stays honest if the list is ever edited.
//
// The list below is **not** a reproduction of EFF's published wordlist — it
// is independently drawn from the same Unix dictionary macOS already ships
// (`/usr/share/dict/words`), filtered to short (3-9 letter), plain, lowercase
// English words and hand-checked for duplicates and stray non-words. That
// sidesteps two problems at once: no network fetch is needed to build it, and
// nothing here reproduces a third party's curated, copyrightable selection —
// it is "EFF-style" (short common words, independent uniform draws, join with
// a separator) rather than EFF's own list.
//
// 1,944 words → each word carries log2(1944) ≈ 10.93 bits. Six words (the
// default) is therefore about 65.6 bits — comfortably past what a determined
// offline attacker can exhaust, and every bit of it is something a human can
// actually read aloud and retype.

const WORDLIST: &[&str] = &[
    "able", "about", "above", "acid", "acorn", "actor", "adapt", "adult",
    "after", "again", "agent", "agree", "ahead", "alarm", "album", "alert",
    "alien", "alike", "alive", "alley", "allow", "almost", "alone", "along",
    "alpha", "alter", "amber", "amount", "ample", "amuse", "anchor", "angel",
    "anger", "angle", "animal", "ankle", "answer", "anthem", "antique", "anvil",
    "apple", "apply", "april", "apron", "arcade", "arch", "arena", "argue",
    "arise", "armor", "army", "around", "arrow", "art", "artist", "ash",
    "ask", "aspect", "asset", "atlas", "atom", "attic", "august", "aunt",
    "author", "auto", "autumn", "avatar", "avenue", "average", "avoid", "await",
    "awake", "award", "aware", "away", "awful", "axis", "baby", "back",
    "bacon", "badge", "bagel", "baker", "balance", "balcony", "ball", "bamboo",
    "banana", "band", "bank", "banner", "barber", "bargain", "barn", "barrel",
    "base", "basic", "basil", "basin", "basket", "batch", "bath", "battle",
    "beach", "beacon", "beam", "bean", "bear", "beast", "beauty", "beaver",
    "become", "bed", "beef", "before", "begin", "behind", "being", "belief",
    "bell", "belong", "below", "belt", "bench", "bend", "berry", "best",
    "better", "beyond", "bicycle", "bike", "bind", "bird", "birth", "bishop",
    "bison", "bitter", "black", "blade", "blame", "blank", "blast", "blaze",
    "bleak", "blend", "bless", "blind", "block", "blood", "bloom", "blossom",
    "blouse", "blue", "blush", "board", "boat", "body", "boil", "bold",
    "bolt", "bone", "bonus", "book", "boost", "boot", "border", "boring",
    "born", "borrow", "boss", "bottle", "bottom", "boulder", "bounce", "bound",
    "bowl", "box", "boxer", "boy", "brain", "brand", "brass", "brave",
    "bread", "break", "breeze", "brick", "bride", "bridge", "brief", "bright",
    "bring", "broad", "broken", "bronze", "broom", "brother", "brown", "brush",
    "bubble", "buddy", "budget", "buffalo", "build", "bulb", "bulk", "bullet",
    "bunch", "bundle", "bunny", "burden", "burger", "burn", "burst", "bus",
    "bush", "business", "busy", "butter", "button", "buyer", "cabin", "cable",
    "cactus", "cage", "cake", "calm", "camera", "camp", "canal", "candy",
    "cannon", "canoe", "canvas", "canyon", "capable", "capital", "captain", "carbon",
    "card", "cargo", "carpet", "carrot", "cart", "carve", "case", "cash",
    "casino", "castle", "casual", "cat", "catch", "cause", "cave", "ceiling",
    "celery", "cell", "cement", "census", "chain", "chair", "chalk", "champion",
    "chance", "change", "chaos", "chapter", "charge", "charm", "chart", "chase",
    "cheap", "check", "cheese", "chef", "cherry", "chest", "chicken", "chief",
    "child", "chill", "chimney", "choice", "choose", "chronic", "chuckle", "chunk",
    "cigar", "circle", "citizen", "city", "civil", "claim", "clarify", "claw",
    "clay", "clean", "clerk", "clever", "click", "cliff", "climb", "clinic",
    "clip", "clock", "close", "cloth", "cloud", "clown", "club", "clump",
    "cluster", "clutch", "coach", "coast", "coconut", "code", "coffee", "coil",
    "coin", "collar", "color", "column", "comfort", "comic", "common", "company",
    "concert", "corn", "corner", "cost", "cotton", "couch", "cough", "country",
    "county", "couple", "courage", "cousin", "cover", "coyote", "crack", "cradle",
    "craft", "crane", "crash", "crater", "crawl", "crazy", "cream", "credit",
    "creek", "crew", "cricket", "crime", "crisp", "critic", "crop", "cross",
    "crouch", "crowd", "crown", "crucial", "cruel", "cruise", "crumble", "crunch",
    "crush", "cry", "crystal", "cube", "culture", "cup", "cupboard", "curious",
    "current", "curtain", "curve", "cushion", "custom", "cute", "cycle", "cypher",
    "damage", "dance", "danger", "daring", "dash", "daughter", "dawn", "day",
    "deal", "debate", "debris", "decade", "december", "decide", "decline", "decorate",
    "decrease", "deer", "defense", "define", "degree", "delay", "delight", "deliver",
    "demand", "denial", "dentist", "deny", "depart", "depend", "deposit", "depth",
    "deputy", "derive", "desert", "design", "desk", "detail", "detect", "device",
    "devote", "diagram", "dial", "diamond", "diary", "dice", "diesel", "diet",
    "differ", "digital", "dilemma", "dinner", "dinosaur", "direct", "dirt", "disagree",
    "discover", "disease", "dish", "dismiss", "disorder", "display", "distance", "divert",
    "divide", "dizzy", "doctor", "document", "dog", "doll", "dolphin", "domain",
    "donate", "donkey", "donor", "door", "dose", "double", "dove", "draft",
    "dragon", "drama", "draw", "dream", "dress", "drift", "drill", "drink",
    "drip", "drive", "drop", "drum", "dry", "duck", "dumb", "dune",
    "during", "dust", "duty", "dwarf", "eager", "eagle", "early", "earn",
    "earth", "easily", "east", "easy", "echo", "ecology", "edge", "edit",
    "educate", "effort", "eight", "either", "elbow", "elder", "eldest", "elegant",
    "element", "elephant", "elevator", "elite", "else", "embark", "embody", "emerge",
    "emotion", "employ", "empty", "enable", "enact", "end", "endless", "endorse",
    "enemy", "energy", "engage", "engine", "enjoy", "enlist", "enough", "enrich",
    "enroll", "ensure", "enter", "entire", "entry", "envelope", "episode", "equal",
    "equip", "erase", "erode", "erosion", "error", "erupt", "escape", "essay",
    "essence", "estate", "eternal", "ethics", "evidence", "evil", "evoke", "exact",
    "example", "excess", "exchange", "excite", "exclude", "excuse", "execute", "exercise",
    "exhaust", "exhibit", "exile", "exist", "exit", "exotic", "expand", "expect",
    "expire", "explain", "expose", "express", "extend", "extra", "eye", "fabric",
    "face", "faculty", "fade", "faint", "faith", "fall", "false", "fame",
    "family", "famous", "fan", "fancy", "fantasy", "farm", "fashion", "fat",
    "father", "fatigue", "fault", "favor", "feature", "february", "federal", "fee",
    "feed", "feel", "female", "fence", "festival", "fetch", "fever", "few",
    "fiber", "fiction", "field", "figure", "file", "film", "filter", "final",
    "find", "finger", "finish", "fire", "firm", "first", "fiscal", "fish",
    "fit", "fitness", "fix", "flag", "flame", "flash", "flat", "flavor",
    "flee", "flight", "flip", "float", "flock", "floor", "flower", "fluid",
    "flush", "fly", "foam", "focus", "fog", "foil", "fold", "follow",
    "food", "foot", "force", "forest", "forget", "fork", "fortune", "forum",
    "forward", "fossil", "foster", "found", "fox", "fragile", "frame", "frank",
    "fraud", "freeze", "fresh", "friend", "fringe", "frog", "front", "frost",
    "frown", "frozen", "fruit", "fuel", "fun", "funny", "fur", "future",
    "gadget", "gain", "galaxy", "game", "gap", "garage", "garbage", "garden",
    "garlic", "garment", "gas", "gate", "gather", "gauge", "gaze", "general",
    "genius", "gentle", "genuine", "gesture", "ghost", "giant", "gift", "giggle",
    "ginger", "giraffe", "girl", "give", "glad", "glance", "glare", "glass",
    "glide", "glimpse", "globe", "gloom", "glory", "glove", "glow", "glue",
    "goat", "goddess", "gold", "good", "goose", "gorilla", "gospel", "gossip",
    "govern", "gown", "grab", "grace", "grain", "grant", "grape", "grass",
    "gravity", "great", "green", "greet", "grid", "grief", "grit", "grocery",
    "group", "grow", "grunt", "guard", "guess", "guide", "guilt", "guitar",
    "gun", "gym", "habit", "hair", "half", "hammer", "hamster", "hand",
    "happy", "harbor", "hard", "harsh", "harvest", "hat", "have", "hawk",
    "hazard", "head", "health", "heart", "heavy", "hedge", "height", "hello",
    "helmet", "help", "hen", "hero", "hidden", "high", "hill", "hint",
    "hip", "hire", "history", "hobby", "hockey", "hold", "hole", "holiday",
    "hollow", "home", "honey", "hood", "hope", "horn", "horse", "hospital",
    "host", "hotel", "hour", "house", "hover", "hub", "huge", "human",
    "humble", "humor", "hundred", "hungry", "hunt", "hurdle", "hurry", "hurt",
    "husband", "hybrid", "ice", "icon", "idea", "identify", "idle", "ignore",
    "ill", "image", "imitate", "immune", "impact", "impose", "improve", "impulse",
    "inch", "include", "income", "increase", "index", "indoor", "infant", "inform",
    "inhale", "inject", "injury", "inmate", "inner", "input", "inquiry", "insane",
    "insect", "inside", "install", "intact", "interest", "into", "invest", "invite",
    "involve", "iron", "island", "isolate", "issue", "item", "ivory", "jacket",
    "jaguar", "jar", "jazz", "jealous", "jeans", "jelly", "jewel", "job",
    "join", "joke", "journey", "joy", "judge", "juice", "jump", "jungle",
    "junior", "junk", "just", "kangaroo", "keen", "keep", "ketchup", "key",
    "kick", "kid", "kidney", "kind", "kingdom", "kiss", "kit", "kitchen",
    "kite", "kitten", "kiwi", "knee", "knife", "knock", "know", "lab",
    "label", "labor", "ladder", "lady", "lake", "lamp", "land", "language",
    "laptop", "large", "later", "latin", "laugh", "laundry", "lava", "law",
    "lawn", "lawsuit", "layer", "lazy", "leader", "leaf", "learn", "leave",
    "lecture", "left", "leg", "legal", "legend", "leisure", "lemon", "lend",
    "length", "lens", "leopard", "lesson", "letter", "level", "liar", "liberty",
    "library", "license", "life", "lift", "light", "like", "limb", "limit",
    "link", "lion", "liquid", "list", "little", "lizard", "load", "loan",
    "lobster", "local", "lock", "logic", "lonely", "long", "loop", "lottery",
    "loud", "lounge", "love", "loyal", "lucky", "luggage", "lumber", "lunar",
    "lunch", "luxury", "lyrics", "machine", "mad", "magic", "maid", "mail",
    "main", "major", "make", "mammal", "man", "mango", "manner", "manual",
    "maple", "marble", "march", "margin", "marine", "market", "maroon", "marriage",
    "mask", "mass", "master", "match", "material", "math", "matrix", "matter",
    "maximum", "maze", "meadow", "mean", "measure", "meat", "mechanic", "medal",
    "media", "melody", "melt", "member", "memory", "mention", "menu", "mercy",
    "merge", "merit", "merry", "mesh", "message", "metal", "method", "mice",
    "middle", "midnight", "mild", "million", "mimic", "mind", "minimum", "minor",
    "minute", "miracle", "mirror", "misery", "miss", "mistake", "mix", "mixed",
    "mixture", "mobile", "model", "modify", "moment", "monitor", "monkey", "month",
    "moon", "moral", "more", "morning", "mosquito", "mother", "motion", "motor",
    "mountain", "mouse", "move", "movie", "much", "muffin", "mule", "multiply",
    "muscle", "museum", "mushroom", "music", "must", "mutual", "myself", "mystery",
    "myth", "naive", "name", "napkin", "narrow", "nasty", "nation", "nature",
    "near", "neck", "need", "negative", "neglect", "neither", "nephew", "nerve",
    "nest", "net", "network", "neutral", "never", "news", "next", "nice",
    "night", "noble", "noise", "nominee", "noodle", "normal", "north", "nose",
    "notable", "note", "nothing", "notice", "novel", "now", "nuclear", "number",
    "nurse", "nut", "oak", "obey", "object", "oblige", "obscure", "observe",
    "obtain", "obvious", "occur", "ocean", "october", "odor", "off", "offer",
    "office", "often", "oil", "okay", "old", "olive", "olympic", "omit",
    "once", "onion", "online", "only", "open", "opera", "opinion", "oppose",
    "option", "orange", "orbit", "orchard", "order", "ordinary", "organ", "orient",
    "original", "orphan", "ostrich", "other", "outdoor", "outer", "output", "outside",
    "oval", "oven", "over", "own", "owner", "oxygen", "oyster", "ozone",
    "pact", "paddle", "page", "pair", "palace", "palm", "panda", "panel",
    "panic", "pants", "paper", "parade", "parent", "park", "parrot", "party",
    "pass", "patch", "path", "patient", "patrol", "pattern", "pause", "pave",
    "payment", "peace", "peanut", "pear", "peasant", "pelican", "pen", "penalty",
    "pencil", "people", "pepper", "perfect", "permit", "person", "pet", "phone",
    "photo", "phrase", "physical", "piano", "picnic", "picture", "piece", "pig",
    "pigeon", "pill", "pilot", "pink", "pioneer", "pipe", "pistol", "pitch",
    "pizza", "place", "planet", "plastic", "plate", "play", "please", "pledge",
    "pluck", "plunge", "poem", "poet", "point", "polar", "pole", "police",
    "pond", "pony", "pool", "popular", "portion", "position", "possible", "post",
    "potato", "pottery", "poverty", "powder", "power", "practice", "praise", "predict",
    "prefer", "prepare", "present", "pretty", "prevent", "price", "pride", "primary",
    "print", "priority", "prison", "private", "prize", "problem", "process", "produce",
    "profit", "program", "project", "promote", "proof", "property", "prosper", "protect",
    "proud", "provide", "public", "pudding", "pull", "pulp", "pulse", "pumpkin",
    "punch", "pupil", "puppy", "purchase", "purity", "purpose", "purse", "push",
    "put", "puzzle", "pyramid", "quality", "quantum", "quarter", "question", "quick",
    "quiet", "quilt", "quit", "quiz", "quote", "rabbit", "raccoon", "race",
    "rack", "radar", "radio", "rail", "rain", "raise", "rally", "ramp",
    "ranch", "random", "range", "rapid", "rare", "rate", "rather", "raven",
    "raw", "razor", "ready", "real", "reason", "rebel", "rebuild", "recall",
    "receive", "recipe", "record", "recycle", "reduce", "reflect", "reform", "refuse",
    "region", "regret", "regular", "reject", "relax", "release", "relief", "rely",
    "remain", "remember", "remind", "remove", "render", "renew", "rent", "repair",
    "repeat", "replace", "report", "require", "rescue", "resemble", "resist", "resource",
    "result", "retire", "retreat", "return", "reunion", "reveal", "review", "reward",
    "rhythm", "rib", "ribbon", "rice", "rich", "riddle", "rifle", "right",
    "rigid", "ring", "riot", "ripple", "rise", "risk", "ritual", "rival",
    "river", "road", "roast", "robot", "rocket", "romance", "roof", "rookie",
    "room", "rose", "rotate", "rough", "round", "route", "royal", "rubber",
    "rude", "rug", "rule", "run", "rural", "sad", "saddle", "sadness",
    "safe", "sail", "salad", "salmon", "salon", "salt", "salute", "same",
    "sample", "sand", "satin", "satisfy", "sauce", "sausage", "save", "say",
    "scale", "scan", "scare", "scatter", "scene", "scheme", "school", "science",
    "scissors", "scorpion", "scout", "scrap", "screen", "script", "scrub", "sea",
    "search", "season", "seat", "second", "secret", "section", "security", "seed",
    "seek", "segment", "select", "sell", "seminar", "senior", "sense", "sentence",
    "series", "service", "session", "settle", "setup", "seven", "shadow", "shaft",
    "shallow", "share", "shed", "shell", "sheriff", "shield", "shift", "shine",
    "ship", "shiver", "shock", "shoe", "shoot", "shop", "shore", "short",
    "shoulder", "shove", "shrimp", "shrug", "shuffle", "shy", "sibling", "sick",
    "side", "siege", "sight", "sign", "silent", "silk", "silly", "silver",
    "similar", "simple", "since", "sing", "siren", "sister", "situate", "six",
    "size", "skate", "sketch", "ski", "skill", "skin", "skirt", "skull",
    "sleep", "slender", "slice", "slide", "slight", "slim", "slogan", "slot",
    "slow", "slush", "small", "smart", "smile", "smoke", "smooth", "snack",
    "snake", "snap", "sniff", "snow", "soap", "soccer", "social", "sock",
    "soda", "soft", "solar", "soldier", "solid", "solve", "someone", "song",
    "soon", "sorry", "sort", "soul", "sound", "soup", "source", "south",
    "space", "spare", "spatial", "spawn", "speak", "special", "speed", "spell",
    "spend", "sphere", "spice", "spider", "spike", "spin", "spirit", "split",
    "spoil", "sponsor", "spoon", "sport", "spot", "spray", "spread", "spring",
    "spy", "square", "squeeze", "squirrel", "stable", "stadium", "staff", "stage",
    "stairs", "stamp", "stand", "start", "state", "stay", "steak", "steel",
    "stem", "step", "stereo", "stick", "still", "sting", "stock", "stomach",
    "stone", "stool", "story", "stove", "strategy", "street", "strike", "strong",
    "struggle", "student", "studio", "study", "stuff", "stumble", "style", "subject",
    "submit", "subway", "success", "such", "sudden", "suffer", "sugar", "suggest",
    "suit", "summer", "sun", "sunny", "sunset", "super", "supply", "sure",
    "surface", "surge", "surprise", "surround", "survey", "suspect", "sustain", "swallow",
    "swamp", "swap", "swarm", "swear", "sweet", "swift", "swim", "swing",
    "switch", "sword", "symbol", "symptom", "syrup", "system", "table", "tackle",
    "tag", "tail", "talent", "talk", "tank", "tape", "target", "task",
    "taste", "tattoo", "taxi", "teach", "team", "tell", "temple", "tenant",
    "tennis", "tent", "term", "test", "text", "thank", "theme", "then",
    "theory", "there", "thing", "this", "thought", "three", "thrive", "throw",
    "thumb", "thunder", "ticket", "tide", "tiger", "tilt", "timber", "time",
    "tiny", "tip", "tired", "tissue", "title", "toast", "tobacco", "today",
    "toddler", "toe", "together", "toilet", "token", "tomato", "tomorrow", "tone",
    "tongue", "tonight", "tool", "tooth", "top", "topic", "topple", "torch",
    "tornado", "tortoise", "toss", "total", "tourist", "toward", "tower", "town",
    "toy", "track", "trade", "traffic", "tragic", "train", "transfer", "trap",
    "trash", "travel", "tray", "treat", "tree", "trend", "trial", "tribe",
    "trick", "trigger", "trim", "trip", "trophy", "trouble", "truck", "true",
    "truly", "trumpet", "trust", "truth", "try", "tube", "tuition", "tumble",
    "tuna", "tunnel", "turkey", "turn", "turtle", "twelve", "twenty", "twice",
    "twin", "twist", "two", "type", "typical", "ugly", "umbrella", "unable",
    "unaware", "uncle", "uncover", "under", "undo", "unfair", "unfold", "unhappy",
    "uniform", "unique", "unit", "universe", "unknown", "unlock", "until", "unusual",
    "unveil", "update", "upgrade", "uphold", "upon", "upper", "upset", "urban",
    "urge", "usage", "use", "used", "useful", "useless", "usual", "utility",
    "vacant", "vacuum", "vague", "valid", "valley", "valve", "van", "vanish",
    "vapor", "various", "vast", "vault", "vehicle", "velvet", "vendor", "venture",
    "venue", "verb", "verify", "version", "very", "vessel", "veteran", "viable",
    "vibrant", "vicious", "victory", "video", "view", "village", "vintage", "violin",
    "virtual", "virus", "visa", "visit", "visual", "vital", "vivid", "vocal",
    "voice", "void", "volcano", "volume", "vote", "voyage", "wage", "wagon",
    "wait", "walk", "wall", "walnut", "want", "warfare", "warm", "warrior",
    "wash", "wasp", "waste", "water", "wave", "way", "wealth", "weapon",
    "wear", "weasel", "weather", "web", "wedding", "weekend", "weird", "welcome",
    "west", "wet", "whale", "wheat", "wheel", "when", "whisper", "wide",
    "width", "wife", "wild", "will", "win", "window", "wine", "wing",
    "wink", "winner", "winter", "wire", "wisdom", "wise", "wish", "witness",
    "wolf", "woman", "wonder", "wood", "wool", "word", "work", "world",
    "worry", "worth", "wrap", "wreck", "wrist", "write", "wrong", "yard",
    "year", "yellow", "you", "young", "youth", "zebra", "zero", "zone",
];

const DEFAULT_PASSPHRASE_WORDS: usize = 6;
const MAX_PASSPHRASE_WORDS: usize = 20;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Passphrase {
    pub phrase: String,
    pub words: usize,
    /// `words * log2(WORDLIST.len())`, computed from the list's real length.
    pub entropy_bits: f64,
}

/// Generate a diceware-style passphrase of `word_count` words joined by `-`.
///
/// `word_count == 0` and anything above [`MAX_PASSPHRASE_WORDS`] are refused
/// rather than silently clamped — a caller that asked for 0 words almost
/// certainly has a bug, and silently "fixing" that hides it.
pub fn generate_passphrase(word_count: usize) -> Result<Passphrase, String> {
    if word_count == 0 {
        return Err("Ask for at least one word.".into());
    }
    if word_count > MAX_PASSPHRASE_WORDS {
        return Err(format!("That's more than {MAX_PASSPHRASE_WORDS} words — pick something typeable."));
    }
    let words: Vec<&str> = (0..word_count).map(|_| WORDLIST[random_index(WORDLIST.len())]).collect();
    let entropy_bits = word_count as f64 * (WORDLIST.len() as f64).log2();
    Ok(Passphrase { phrase: words.join("-"), words: word_count, entropy_bits })
}

/// Palette-facing wrapper: generate with the default word count and put it on
/// the clipboard via the usual `ToolOutcome::copied` path (the frontend does
/// the actual clipboard write, same as every other `dev.rs` tool).
pub fn passphrase_outcome(word_count: Option<usize>) -> ToolOutcome {
    match generate_passphrase(word_count.unwrap_or(DEFAULT_PASSPHRASE_WORDS)) {
        Ok(p) => ToolOutcome::copied(
            p.phrase,
            format!("{} words, ~{} bits of entropy", p.words, p.entropy_bits.round() as i64),
        ),
        Err(e) => ToolOutcome::err(e),
    }
}

// ---------------------------------------------------------------------------
// 2. Clipboard auto-clear timer (item 117)
// ---------------------------------------------------------------------------
//
// # What this needs from `clipboard/` that it does not have
//
// This operates on the **live OS clipboard** via `arboard` directly — the
// same crate `clipboard/watcher.rs` and `popbar.rs` already use — not on
// clipboard *history* in `clipboard/store.rs`, which is off limits here. That
// means:
//
// * Arming a timer after copying something (a generated password, a
//   passphrase, anything else Caduceus puts on the clipboard itself) works
//   today, with nothing else to change.
// * It does **not** reach anything a user copies by hand outside Caduceus, and
//   it does not stop that copy from also landing in clipboard history as
//   plaintext. Doing that needs a hook in `clipboard/watcher.rs` (something
//   like "mark this entry sensitive, purge it after N seconds") which is
//   `clipboard/`'s call to make, not this file's — reported here rather than
//   worked around.
//
// # Cancellation
//
// A monotonic generation counter, not a `JoinHandle` — arming a second timer
// (or explicitly cancelling) bumps the counter, and the sleeping thread
// checks it before acting. That means an in-flight timer from a copy the user
// has since overwritten just no-ops instead of clearing whatever is on the
// clipboard *now*. The thread also re-checks the clipboard's actual contents
// against a snapshot taken at arm time before clearing, as a second guard
// against wiping something unrelated the user copied in between.

static CLEAR_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Arm an auto-clear: after `seconds`, wipe the clipboard *if* it still holds
/// exactly what was on it when this was called and nothing has superseded
/// this arm.
pub fn arm_clipboard_auto_clear(seconds: u64) -> ToolOutcome {
    if seconds == 0 {
        return ToolOutcome::err("Auto-clear needs a positive number of seconds.");
    }
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => return ToolOutcome::err(format!("Clipboard unavailable: {e}")),
    };
    let snapshot = match clipboard.get_text() {
        Ok(t) => t,
        Err(e) => return ToolOutcome::err(format!("Nothing text-based is on the clipboard to arm: {e}")),
    };

    let generation = CLEAR_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(seconds));
        if CLEAR_GENERATION.load(Ordering::SeqCst) != generation {
            return; // superseded by a newer arm or an explicit cancel
        }
        let Ok(mut clipboard) = arboard::Clipboard::new() else { return };
        if clipboard.get_text().map(|t| t == snapshot).unwrap_or(false) {
            let _ = clipboard.clear();
        }
    });

    ToolOutcome::ok(format!(
        "The clipboard will clear automatically in {seconds}s, unless you copy something else first."
    ))
}

/// Cancel any pending auto-clear. Reversible by construction: this only ever
/// stops a *future* clear, it never touches the clipboard's current contents.
pub fn cancel_clipboard_auto_clear() -> ToolOutcome {
    CLEAR_GENERATION.fetch_add(1, Ordering::SeqCst);
    ToolOutcome::ok("Auto-clear cancelled.")
}

// ---------------------------------------------------------------------------
// 4. Microphone mute toggle (item 119)
// ---------------------------------------------------------------------------
//
// macOS's AppleScript Standard Additions expose `output muted of (get volume
// settings)` — a real boolean `system.rs`'s `ToggleMute` already flips — but
// there is no `input muted` counterpart. Verified against this machine:
//
// ```text
// $ osascript -e 'get volume settings'
// output volume:50, input volume:44, alert volume:33, output muted:false
// ```
//
// Four properties, and input is the one without a "muted" partner. So "mute"
// here means "remember the current input volume, then set it to 0"; "unmute"
// means "set it back". The previous level is kept in an in-process `Mutex`,
// not on disk — it is not a secret, but it is also not state that needs to
// survive an app restart, and persisting it would mean touching `settings/`.
// If Caduceus restarts while muted, unmuting falls back to a reasonable
// default (50%) and says so, rather than claiming to restore a value it no
// longer has.

const FALLBACK_INPUT_VOLUME: i32 = 50;

static SAVED_INPUT_VOLUME: Mutex<Option<i32>> = Mutex::new(None);

fn parse_input_volume(raw: &str) -> Result<i32, String> {
    raw.trim().parse::<i32>().map_err(|_| format!("Could not read the input volume from: {raw:?}"))
}

/// Current system input (microphone) volume, 0-100.
pub fn mic_input_volume() -> Result<i32, String> {
    super::apple::run_script("input volume of (get volume settings)").and_then(|s| parse_input_volume(&s))
}

/// Best-effort "is the mic muted" — true exactly when the input volume is 0,
/// whether or not Caduceus is the one that put it there.
pub fn mic_muted() -> Result<bool, String> {
    Ok(mic_input_volume()? <= 0)
}

/// Mute or unmute the microphone by zeroing/restoring the input volume.
pub fn set_mic_muted(mute: bool) -> ToolOutcome {
    if mute {
        let current = match mic_input_volume() {
            Ok(v) => v,
            Err(e) => return ToolOutcome::err(e),
        };
        if current <= 0 {
            return ToolOutcome::ok("The microphone is already muted.");
        }
        *SAVED_INPUT_VOLUME.lock() = Some(current);
        match super::apple::run_script("set volume input volume 0") {
            Ok(_) => ToolOutcome::ok(format!("Microphone muted (input volume was {current}%).")),
            Err(e) => ToolOutcome::err(format!("Could not mute the microphone: {e}")),
        }
    } else {
        let restore = SAVED_INPUT_VOLUME.lock().take().unwrap_or(FALLBACK_INPUT_VOLUME);
        match super::apple::run_script(&format!("set volume input volume {restore}")) {
            Ok(_) => ToolOutcome::ok(format!("Microphone unmuted, input volume restored to {restore}%.")),
            Err(e) => ToolOutcome::err(format!("Could not unmute the microphone: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Camera & microphone activity log (item 120)
// ---------------------------------------------------------------------------
//
// # Verified on this machine (macOS 26.5.2, unified logging, no elevated
// privileges)
//
// `log show --predicate 'subsystem == "com.apple.TCC"' --last 1h --style
// compact` (via `/usr/bin/log`, not the zsh `log` builtin that shadows it in
// an interactive shell — irrelevant to `Command::new`, which never goes
// through a shell) produces real, attributable rows as a normal user. A
// microphone check, captured live during this investigation:
//
// ```text
// 2026-07-27 23:23:31.533 Df tccd[25991:28d4021] [com.apple.TCC:access] \
//   AUTHREQ_CTX: msgID=77469.2, function=<private>, \
//   service=kTCCServiceMicrophone, preflight=yes, query=1, ...
// ```
//
// and, correlated by the same `msgID` a few lines later in the same request:
//
// ```text
// AUTHREQ_ATTRIBUTION: msgID=12228.1, attribution={ \
//   responsible={TCCDProcess: identifier=com.anthropic.claude-code, ...}, \
//   accessing={TCCDProcess: identifier=com.caduceus.desktop.native-helper, ...}, }
// ```
//
// Aggregating `accessing={TCCDProcess: identifier=...` across an hour on this
// machine also turned up `com.apple.contacts.postersyncd`, `com.apple.calaccessd`,
// `com.macparakeet.MacParakeet` and others — i.e. this is not a microphone/
// camera-only feed, TCC logs *every* permission-gated service, which is
// exactly why the predicate below filters to `AUTHREQ_CTX`/`AUTHREQ_ATTRIBUTION`
// lines and the parser below throws away every service except
// `kTCCServiceMicrophone` and `kTCCServiceCamera`.
//
// `function=<private>` shows some fields are redacted for privacy under
// normal logging config — but the `identifier=`/`binary_path=` fields this
// parser reads are not, so no elevated `log config --mode private_data:on` is
// needed.
//
// # Why two regexes and a join
//
// A single TCC access check emits its service (`AUTHREQ_CTX`) and its
// requesting process (`AUTHREQ_ATTRIBUTION`) on *separate* lines, tied
// together only by a shared `msgID`. This does two passes over the log text —
// build a `msgID -> service` map and a `msgID -> (app, timestamp)` map, then
// join them — rather than trying to match a single regex across lines.
//
// # Why events get collapsed
//
// A single moment of "app X touched the mic" routinely produces eight or nine
// `AUTHREQ_CTX`/`AUTHREQ_ATTRIBUTION` pairs a few milliseconds apart (visible
// in the raw capture above: `msgID=77469.2` through `.23` inside one second).
// Reporting all of them would read like the mic was toggled nine times in a
// blink. Entries from the same app and service within the same second are
// collapsed to one.

const LOG_QUERY_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    /// `YYYY-MM-DD HH:MM:SS.mmm`, straight from the log line.
    pub timestamp: String,
    /// The requesting process's bundle/service identifier, e.g. `com.zoom.xos`.
    pub app: String,
    /// `"Microphone"` or `"Camera"`.
    pub service: String,
}

fn ctx_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"AUTHREQ_CTX: msgID=([0-9]+\.[0-9]+),.*?service=(kTCCServiceMicrophone|kTCCServiceCamera)")
            .expect("static pattern is valid")
    })
}

fn attribution_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"AUTHREQ_ATTRIBUTION: msgID=([0-9]+\.[0-9]+),.*?(?:accessing|requesting)=\{TCCDProcess: identifier=([^,]+)",
        )
        .expect("static pattern is valid")
    })
}

/// Parse `log show --style compact` TCC output into deduplicated camera/mic
/// events. Pure function of the text — no subprocess, no clock — so it is
/// unit-testable against a captured fixture without touching the real log.
fn parse_tcc_activity(raw: &str) -> Vec<ActivityEvent> {
    let mut service_by_id: HashMap<String, &'static str> = HashMap::new();
    let mut app_by_id: HashMap<String, String> = HashMap::new();
    let mut timestamp_by_id: HashMap<String, String> = HashMap::new();

    for line in raw.lines() {
        if let Some(caps) = ctx_regex().captures(line) {
            let id = caps[1].to_string();
            let service = match &caps[2] {
                "kTCCServiceMicrophone" => "Microphone",
                "kTCCServiceCamera" => "Camera",
                _ => continue,
            };
            service_by_id.insert(id, service);
        } else if let Some(caps) = attribution_regex().captures(line) {
            let id = caps[1].to_string();
            app_by_id.insert(id.clone(), caps[2].trim().to_string());
            let mut tokens = line.split_whitespace();
            if let (Some(date), Some(time)) = (tokens.next(), tokens.next()) {
                timestamp_by_id.insert(id, format!("{date} {time}"));
            }
        }
    }

    let mut events: Vec<ActivityEvent> = service_by_id
        .into_iter()
        .filter_map(|(id, service)| {
            let app = app_by_id.get(&id)?.clone();
            let timestamp = timestamp_by_id.get(&id).cloned().unwrap_or_default();
            Some(ActivityEvent { timestamp, app, service: service.to_string() })
        })
        .collect();

    events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    dedupe_bursts(events)
}

/// Collapse same app+service events that land in the same second.
fn dedupe_bursts(events: Vec<ActivityEvent>) -> Vec<ActivityEvent> {
    let mut seen: HashSet<(String, String, String)> = HashSet::new();
    let mut out = Vec::with_capacity(events.len());
    for event in events {
        let second = event.timestamp.get(0..19).unwrap_or(&event.timestamp).to_string();
        if seen.insert((event.app.clone(), event.service.clone(), second)) {
            out.push(event);
        }
    }
    out
}

/// Camera/microphone activity over the last `minutes` minutes, newest first.
///
/// Clamped to 1-1440 minutes (a day): unbounded windows on a system that has
/// been up for weeks make `log show` slow enough to risk the timeout below,
/// and "activity in the last 3 weeks" is not what this feature is for.
pub fn recent_camera_mic_activity(minutes: u32) -> Result<Vec<ActivityEvent>, String> {
    let minutes = minutes.clamp(1, 1440);
    let predicate = "subsystem == \"com.apple.TCC\" AND (eventMessage CONTAINS \"AUTHREQ_CTX\" \
                      OR eventMessage CONTAINS \"AUTHREQ_ATTRIBUTION\")";
    let window = format!("{minutes}m");

    // `/usr/bin/log` by full path — not because `Command::new("log")` would
    // resolve to zsh's builtin (it would not; `Command` never goes through a
    // shell), but so the intent reads the same way in code as it does on a
    // terminal where that builtin genuinely does shadow it.
    let out = output_with_timeout(
        Command::new("/usr/bin/log").args(["show", "--predicate", predicate, "--last", &window, "--style", "compact"]),
        LOG_QUERY_TIMEOUT,
        "The system log did not answer in time.",
    )?;
    if !out.status.success() {
        return Err(format!("log show failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(parse_tcc_activity(&String::from_utf8_lossy(&out.stdout)))
}

/// Palette-facing summary of the above.
pub fn camera_mic_activity_outcome(minutes: u32) -> ToolOutcome {
    match recent_camera_mic_activity(minutes) {
        Ok(events) if events.is_empty() => {
            ToolOutcome::ok(format!("No microphone or camera activity in the last {} minutes.", minutes.clamp(1, 1440)))
        }
        Ok(events) => {
            let lines: Vec<String> =
                events.iter().take(25).map(|e| format!("{}  {} — {}", e.timestamp, e.service, e.app)).collect();
            ToolOutcome::ok(lines.join("\n"))
        }
        Err(e) => ToolOutcome::err(e),
    }
}

// ---------------------------------------------------------------------------
// 6. Network firewall quick switch (item 121)
// ---------------------------------------------------------------------------
//
// Reading `socketfilterfw --getglobalstate` needs no privilege — verified on
// this machine as the logged-in user, no `sudo`:
//
// ```text
// $ /usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate
// Firewall is disabled. (State = 0)
// ```
//
// *Changing* it does need an admin password — the firewall toggle in System
// Settings is behind an authenticate button precisely because it is backed by
// a privileged XPC service. This file does **not** shell out to `sudo`
// `socketfilterfw --setglobalstate` and does not implement any password
// prompt of its own — either would mean Caduceus asking for (or worse,
// storing) an admin credential outside the OS's own trusted authentication
// UI, which is exactly the failure mode admin-gated settings exist to
// prevent. Instead, changing it opens System Settings straight to the
// Firewall pane — verified working on this machine (`osascript -e 'open
// location "x-apple.systempreferences:com.apple.preference.security?Firewall"'`
// brought System Settings to the front) — and lets the user authenticate
// through Apple's own dialog, exactly as if they had clicked there
// themselves.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FirewallState {
    On,
    Off,
}

fn parse_firewall_state(text: &str) -> Result<FirewallState, String> {
    let lower = text.to_lowercase();
    if lower.contains("enabled") {
        Ok(FirewallState::On)
    } else if lower.contains("disabled") {
        Ok(FirewallState::Off)
    } else {
        Err(format!("Could not tell whether the firewall is on from: {text:?}"))
    }
}

/// Read-only: whether the macOS application firewall is currently on.
pub fn firewall_state() -> Result<FirewallState, String> {
    let out = output_with_timeout(
        Command::new("/usr/libexec/ApplicationFirewall/socketfilterfw").arg("--getglobalstate"),
        TOOL_TIMEOUT,
        "socketfilterfw did not answer in time.",
    )?;
    parse_firewall_state(&String::from_utf8_lossy(&out.stdout))
}

/// Send the user to System Settings → Firewall to make the actual change.
///
/// Two URLs, tried in order — the same "old pane id, then new pane id"
/// fallback `shortcuts::exec::open_settings_pane` already uses for other
/// Privacy & Security panes, reimplemented here rather than by adding a
/// `"firewall"` case to that allow-list, since `shortcuts/` is not this
/// file's to edit. Worth wiring into that allow-list later — see the header
/// comment's wrapper list.
pub fn open_firewall_settings() -> ToolOutcome {
    const URLS: &[&str] = &[
        // Verified working on this machine (macOS 26.5.2).
        "x-apple.systempreferences:com.apple.preference.security?Firewall",
        // Unverified fallback for a future System Settings bundle id change,
        // mirroring the pattern already used for the Privacy panes.
        "x-apple.systempreferences:com.apple.Security-Settings.extension?Firewall",
    ];
    for url in URLS {
        let opened = output_with_timeout(Command::new("open").arg(url), TOOL_TIMEOUT, "open did not answer in time.")
            .map(|out| out.status.success())
            .unwrap_or(false);
        if opened {
            return ToolOutcome::ok(
                "Opened System Settings → Firewall. Turning it on or off there will ask you to authenticate.",
            );
        }
    }
    ToolOutcome::err("Could not open System Settings.")
}

// ---------------------------------------------------------------------------
// 7. App lock with TouchID (item 122) — documented gap, not half-built
// ---------------------------------------------------------------------------

/// Always `false`: a real answer, not a stub pretending to be one.
///
/// TouchID authentication on macOS goes through the LocalAuthentication
/// framework (`LAContext.evaluatePolicy`). Reaching it needs either an
/// Objective-C binding for that framework or a small native helper — neither
/// exists in this project. Checked before writing this:
///
/// * `Cargo.toml` has `objc2`, `objc2-app-kit`, `objc2-foundation` for macOS —
///   no `objc2-local-authentication`.
/// * `Cargo.lock` was searched directly for it too, in case some other
///   dependency pulled it in transitively (the way `sha2`/`hkdf`/`hmac` are
///   present transitively via TLS, for instance) — it is not there at any
///   version, transitively or otherwise. Nothing in this dependency graph has
///   ever linked LocalAuthentication.
/// * There is no command-line entry point to a TouchID prompt either. `sudo`
///   can be configured to accept TouchID via `pam_tid.so`, but driving that
///   from here would mean shelling out to `sudo`, which item 121's
///   constraints already rule out for the same underlying reason: Caduceus
///   should never be the thing standing between a user and an OS auth
///   prompt.
///
/// Building "App lock" without real biometric verification behind it — a
/// toggle that just remembers a flag — would be worse than not building it:
/// it would look like a security feature while providing none. This function
/// exists so a caller can show "not available on this build" honestly rather
/// than the UI silently having no app-lock entry at all.
pub fn touch_id_available() -> bool {
    false
}

// ---------------------------------------------------------------------------
// 8. File vault lockbox (item 123)
// ---------------------------------------------------------------------------
//
// # Construction
//
// Reuses `clipboard::crypto::{encrypt, decrypt}` exactly as clipboard history
// does: ChaCha20-Poly1305 (AEAD), a fresh random 96-bit nonce per file, output
// laid out as `nonce || ciphertext || tag`. Nothing about the AEAD is
// reinvented — this is the same call, on file bytes instead of clipboard rows.
//
// On top of that blob this file prepends a 5-byte header (`CDVL` + a version
// byte) so a `.vault` file is self-describing and a future format change has
// somewhere to branch on.
//
// # The part that is *not* reused: turning a passphrase into a key
//
// `clipboard::crypto` never has this problem — its key is 32 random bytes
// from `getrandom`, straight into the cipher (see `settings::secrets::
// get_or_create_clipboard_key`). A human passphrase is not 32 random bytes,
// and turning one into a key safely is normally Argon2id or PBKDF2-HMAC-SHA256
// — a slow, salted, iterated hash specifically designed to make guessing
// expensive. **None of that is available here.** This crate has no hash
// function as a direct dependency at all — not sha2, not blake2, nothing —
// and `chacha20poly1305`/`getrandom`/`base64` provide no path to one. (`sha2`,
// `hkdf` and `hmac` do exist *transitively*, pulled in by TLS, but promoting a
// transitive crate to a direct one is still adding a dependency in every way
// that matters here — new lines in `Cargo.toml`, a new supply-chain surface
// to review — so it was not done. That edit is also to a file outside this
// agent's ownership.)
//
// Hand-rolling a KDF substitute (iterated hashing with what's on hand, a
// home-made stretching loop, anything claiming to add work-factor) is
// precisely "inventing your own crypto construction" — the one thing the
// task brief rules out explicitly, and for good reason: home-made KDFs are
// where most amateur crypto goes wrong.
//
// So this does neither: **no key-stretching is attempted, and none is
// implied.** [`key_from_passphrase`] is deliberately just byte formatting —
// the passphrase's UTF-8 bytes tiled to fill 32 bytes, nothing hashed, nothing
// iterated. That is exactly as secure as the passphrase's own entropy and not
// one bit more: cracking it costs an attacker one guess per candidate
// passphrase, not the thousands of guesses a real KDF would force. A minimum
// length is enforced as a floor, and the passphrase generator above (which
// produces ~65 bits of entropy by default) is the recommended way to produce
// one, but a 12-character floor is still far short of what Argon2id would
// provide. **This is a real limitation of the no-new-dependency constraint,
// not a hidden one** — flagged here, in the lock/unlock error text, and in
// this agent's final report, rather than dressed up as more than it is.

const VAULT_MAGIC: &[u8; 4] = b"CDVL";

/// Format 2. Version 1 derived the key by tiling the passphrase's own bytes —
/// no stretching at all, so the key was exactly as strong as the passphrase and
/// an attacker with the file could try candidates as fast as they could read
/// them. That is not what the word "vault" promises, so the format now carries
/// a random salt and the key comes from Argon2id.
///
/// Nothing reads version 1: it was never released, so there is no file in the
/// world to stay compatible with, and a compatibility path would only keep the
/// weak derivation alive.
const VAULT_VERSION: u8 = 2;
const VAULT_SALT_LEN: usize = 16;
const VAULT_HEADER_LEN: usize = 5 + VAULT_SALT_LEN;
const MIN_PASSPHRASE_LEN: usize = 12;

/// Tile a passphrase's bytes to fill exactly [`crypto::KEY_LEN`] bytes.
///
/// Deliberately not a hash or a KDF — see the module section above for why.
/// Stretch a passphrase into a key with Argon2id.
///
/// Argon2id and not a plain hash, because the threat here is someone holding
/// the `.vault` file and guessing offline at whatever rate their hardware
/// allows. A KDF's whole job is to make each guess cost real time and real
/// memory; a bare hash makes it cost neither.
///
/// The defaults are the `argon2` crate's own recommended parameters, which
/// target roughly 19 MB and a few hundred milliseconds. That delay is the
/// feature — it is paid once when you lock or unlock a file, and paid again by
/// an attacker on every single candidate they try.
fn key_from_passphrase(
    passphrase: &str,
    salt: &[u8; VAULT_SALT_LEN],
) -> Result<[u8; crypto::KEY_LEN], String> {
    use argon2::Argon2;

    let bytes = passphrase.as_bytes();
    if bytes.len() < MIN_PASSPHRASE_LEN {
        return Err(format!(
            "That passphrase is too short ({} characters; {MIN_PASSPHRASE_LEN} minimum). Use the \
             passphrase generator for something long and random.",
            bytes.len()
        ));
    }

    let mut key = [0u8; crypto::KEY_LEN];
    Argon2::default()
        .hash_password_into(bytes, salt, &mut key)
        .map_err(|e| format!("Could not derive a key from that passphrase: {e}"))?;
    Ok(key)
}

/// A fresh random salt for a new vault file.
///
/// Per file, never reused: two files locked with the same passphrase must not
/// share a key, or cracking one cracks both.
fn new_salt() -> Result<[u8; VAULT_SALT_LEN], String> {
    let mut salt = [0u8; VAULT_SALT_LEN];
    getrandom::fill(&mut salt).map_err(|e| format!("Could not generate a salt: {e}"))?;
    Ok(salt)
}

/// Encrypt `path` in place into a sibling `<name>.vault` file.
///
/// Never overwrites an existing `.vault` file — if one is already there this
/// refuses rather than clobbering it, so a caller always knows exactly what
/// changed. `delete_original` is opt-in and explicit: encrypting without
/// removing the source is non-destructive and fully reversible by just
/// deleting the `.vault` copy; deleting the source is a second, separate
/// decision a caller has to make on purpose, not a default this file picks
/// for them.
pub fn lock_file(path: &Path, passphrase: &str, delete_original: bool) -> Result<PathBuf, String> {
    if !path.is_file() {
        return Err("That file does not exist.".into());
    }
    let salt = new_salt()?;
    let mut key = key_from_passphrase(passphrase, &salt)?;

    let mut plaintext = std::fs::read(path).map_err(|e| format!("Could not read the file: {e}"))?;
    let blob = crypto::encrypt(&key, &plaintext).map_err(|e| format!("Encryption failed: {e}"))?;

    // Best-effort scrub: overwrite the plaintext and key bytes once they are
    // no longer needed. Not a guarantee — the compiler is free to reorder or
    // elide a plain write it can prove is dead, and this does nothing about
    // copies the allocator or OS paging already made — just strictly better
    // than leaving an obvious secret sitting in the heap for the rest of the
    // process's life for no reason.
    plaintext.iter_mut().for_each(|b| *b = 0);
    key.iter_mut().for_each(|b| *b = 0);

    let mut dest_name = path.as_os_str().to_os_string();
    dest_name.push(".vault");
    let dest = PathBuf::from(dest_name);
    if dest.exists() {
        return Err(format!("{} already exists — remove or rename it first.", dest.display()));
    }

    let mut out = Vec::with_capacity(VAULT_HEADER_LEN + blob.len());
    out.extend_from_slice(VAULT_MAGIC);
    out.push(VAULT_VERSION);
    // The salt is not a secret — it exists so two files never share a key —
    // so it travels in the clear alongside the ciphertext it belongs to.
    out.extend_from_slice(&salt);
    out.extend_from_slice(&blob);
    std::fs::write(&dest, &out).map_err(|e| format!("Could not write the vault file: {e}"))?;

    if delete_original {
        std::fs::remove_file(path)
            .map_err(|e| format!("Wrote {} but could not remove the original: {e}", dest.display()))?;
    }

    Ok(dest)
}

/// Decrypt a `.vault` file written by [`lock_file`] back to plaintext.
///
/// Errors from a wrong passphrase and errors from a corrupted/tampered file
/// are reported with the same message on purpose, same reasoning as
/// `clipboard::crypto::CryptoError::Decrypt`: telling the two apart leaks
/// information to whoever is trying passphrases.
pub fn unlock_file(path: &Path, passphrase: &str) -> Result<PathBuf, String> {
    if !path.is_file() {
        return Err("That file does not exist.".into());
    }
    let raw = std::fs::read(path).map_err(|e| format!("Could not read the file: {e}"))?;
    if raw.len() < VAULT_HEADER_LEN || &raw[0..4] != VAULT_MAGIC {
        return Err("That is not a Caduceus vault file.".into());
    }
    if raw[4] != VAULT_VERSION {
        return Err(format!("This vault uses a newer format ({}) than this version of Caduceus understands.", raw[4]));
    }

    let mut salt = [0u8; VAULT_SALT_LEN];
    salt.copy_from_slice(&raw[5..VAULT_HEADER_LEN]);
    let mut key = key_from_passphrase(passphrase, &salt)?;
    let decrypt_result = crypto::decrypt(&key, &raw[VAULT_HEADER_LEN..]);
    key.iter_mut().for_each(|b| *b = 0);
    let mut plaintext =
        decrypt_result.map_err(|_| "Could not unlock it — wrong passphrase, or the file is corrupted.".to_string())?;

    let dest = match path.to_str().and_then(|s| s.strip_suffix(".vault")) {
        Some(stripped) => PathBuf::from(stripped),
        None => {
            let mut fallback = path.as_os_str().to_os_string();
            fallback.push(".decrypted");
            PathBuf::from(fallback)
        }
    };
    if dest.exists() {
        return Err(format!("{} already exists — remove or rename it first.", dest.display()));
    }
    std::fs::write(&dest, &plaintext).map_err(|e| format!("Could not write the decrypted file: {e}"))?;
    plaintext.iter_mut().for_each(|b| *b = 0);

    Ok(dest)
}

/// Palette-facing wrapper for [`lock_file`].
pub fn lock_file_outcome(path: &str, passphrase: &str, delete_original: bool) -> ToolOutcome {
    match lock_file(Path::new(path), passphrase, delete_original) {
        Ok(dest) => ToolOutcome::ok(format!(
            "Locked. Wrote {}{}",
            dest.display(),
            if delete_original { " and removed the original." } else { " (original kept)." }
        )),
        Err(e) => ToolOutcome::err(e),
    }
}

/// Palette-facing wrapper for [`unlock_file`].
pub fn unlock_file_outcome(path: &str, passphrase: &str) -> ToolOutcome {
    match unlock_file(Path::new(path), passphrase) {
        Ok(dest) => ToolOutcome::ok(format!("Unlocked to {}", dest.display())),
        Err(e) => ToolOutcome::err(e),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// Nothing below touches Finder, the firewall, the microphone, the real
// clipboard, or spawns `/usr/bin/log` against the live system log — every
// system-mutating or environment-dependent function above (`set_mic_muted`,
// `open_firewall_settings`, `recent_camera_mic_activity`,
// `arm_clipboard_auto_clear`) is exercised only through the pure parsing/logic
// helpers it is built from (`parse_firewall_state`, `parse_tcc_activity`,
// `parse_input_volume`, `key_from_passphrase`), plus the fully self-contained
// pieces (the passphrase generator, the file vault, which only touches files
// this test creates in a temp directory and cleans up itself).

#[cfg(test)]
mod tests {
    use super::*;

    // -- wordlist ---------------------------------------------------------

    #[test]
    fn wordlist_has_no_duplicates() {
        let unique: HashSet<&str> = WORDLIST.iter().copied().collect();
        assert_eq!(unique.len(), WORDLIST.len(), "duplicate word in WORDLIST");
    }

    #[test]
    fn wordlist_is_lowercase_ascii_alphabetic_and_nonempty() {
        for word in WORDLIST {
            assert!(!word.is_empty());
            assert!(
                word.chars().all(|c| c.is_ascii_lowercase()),
                "{word:?} is not plain lowercase ascii"
            );
        }
    }

    // -- passphrase generator ---------------------------------------------

    #[test]
    fn generate_passphrase_returns_the_requested_word_count() {
        let p = generate_passphrase(6).unwrap();
        assert_eq!(p.words, 6);
        assert_eq!(p.phrase.split('-').count(), 6);
    }

    #[test]
    fn generate_passphrase_entropy_matches_the_formula() {
        let p = generate_passphrase(4).unwrap();
        let expected = 4.0 * (WORDLIST.len() as f64).log2();
        assert!((p.entropy_bits - expected).abs() < 1e-9);
    }

    #[test]
    fn generate_passphrase_rejects_zero_words() {
        assert!(generate_passphrase(0).is_err());
    }

    #[test]
    fn generate_passphrase_rejects_absurd_word_counts() {
        assert!(generate_passphrase(MAX_PASSPHRASE_WORDS + 1).is_err());
    }

    #[test]
    fn every_word_in_a_passphrase_comes_from_the_list() {
        let p = generate_passphrase(10).unwrap();
        for word in p.phrase.split('-') {
            assert!(WORDLIST.contains(&word), "{word:?} is not in WORDLIST");
        }
    }

    #[test]
    fn random_index_of_one_item_is_always_that_item() {
        for _ in 0..100 {
            assert_eq!(random_index(1), 0);
        }
    }

    // -- microphone volume parsing -----------------------------------------

    #[test]
    fn parses_a_plain_volume_reading() {
        assert_eq!(parse_input_volume("44\n"), Ok(44));
        assert_eq!(parse_input_volume("0"), Ok(0));
    }

    #[test]
    fn rejects_a_reading_that_is_not_a_number() {
        assert!(parse_input_volume("not a number").is_err());
    }

    // -- firewall state parsing ---------------------------------------------

    #[test]
    fn parses_firewall_enabled() {
        assert_eq!(parse_firewall_state("Firewall is enabled. (State = 1)"), Ok(FirewallState::On));
    }

    #[test]
    fn parses_firewall_disabled() {
        assert_eq!(parse_firewall_state("Firewall is disabled. (State = 0)"), Ok(FirewallState::Off));
    }

    #[test]
    fn unrecognized_firewall_output_is_an_error_not_a_guess() {
        assert!(parse_firewall_state("something unexpected").is_err());
    }

    // -- TCC activity log parsing --------------------------------------------

    /// A trimmed-down, shape-accurate fixture based on lines actually
    /// captured from this machine's unified log while investigating item
    /// 120 (see the module doc comment for the untrimmed originals). Two
    /// requests: one microphone check from `com.caduceus.desktop.native-helper`
    /// that fires the CTX/ATTRIBUTION pair twice a few milliseconds apart
    /// (the burst this parser is expected to collapse), and one camera check
    /// from a different app a minute later.
    const TCC_FIXTURE: &str = concat!(
        "2026-07-27 23:23:31.533 Df tccd[1:1] [com.apple.TCC:access] AUTHREQ_CTX: msgID=1.1, function=<private>, service=kTCCServiceMicrophone, preflight=yes, query=1, client_dict=(null), daemon_dict=<private>\n",
        "2026-07-27 23:23:31.534 Df tccd[1:1] [com.apple.TCC:access] AUTHREQ_ATTRIBUTION: msgID=1.1, attribution={accessing={TCCDProcess: identifier=com.caduceus.desktop.native-helper, pid=12228, },},\n",
        "2026-07-27 23:23:31.611 Df tccd[1:1] [com.apple.TCC:access] AUTHREQ_CTX: msgID=1.2, function=<private>, service=kTCCServiceMicrophone, preflight=yes, query=1, client_dict=(null), daemon_dict=<private>\n",
        "2026-07-27 23:23:31.612 Df tccd[1:1] [com.apple.TCC:access] AUTHREQ_ATTRIBUTION: msgID=1.2, attribution={accessing={TCCDProcess: identifier=com.caduceus.desktop.native-helper, pid=12228, },},\n",
        "2026-07-27 23:24:40.201 Df tccd[1:1] [com.apple.TCC:access] AUTHREQ_CTX: msgID=2.1, function=<private>, service=kTCCServiceCamera, preflight=yes, query=1, client_dict=(null), daemon_dict=<private>\n",
        "2026-07-27 23:24:40.202 Df tccd[1:1] [com.apple.TCC:access] AUTHREQ_ATTRIBUTION: msgID=2.1, attribution={accessing={TCCDProcess: identifier=us.zoom.xos, pid=555, },},\n",
        "2026-07-27 23:24:40.300 Df tccd[1:1] [com.apple.TCC:access] AUTHREQ_CTX: msgID=2.2, function=<private>, service=kTCCServiceSystemPolicyAppBundles, preflight=yes, query=1, client_dict=(null), daemon_dict=<private>\n",
        "2026-07-27 23:24:40.301 Df tccd[1:1] [com.apple.TCC:access] AUTHREQ_ATTRIBUTION: msgID=2.2, attribution={accessing={TCCDProcess: identifier=com.apple.controlcenter, pid=1069, },},\n",
    );

    #[test]
    fn parses_microphone_and_camera_events_with_their_apps() {
        let events = parse_tcc_activity(TCC_FIXTURE);
        assert!(events.iter().any(|e| e.app == "com.caduceus.desktop.native-helper" && e.service == "Microphone"));
        assert!(events.iter().any(|e| e.app == "us.zoom.xos" && e.service == "Camera"));
    }

    #[test]
    fn ignores_non_camera_non_microphone_tcc_services() {
        let events = parse_tcc_activity(TCC_FIXTURE);
        assert!(!events.iter().any(|e| e.app == "com.apple.controlcenter"));
    }

    #[test]
    fn collapses_a_same_second_burst_into_one_event() {
        let events = parse_tcc_activity(TCC_FIXTURE);
        let mic_events =
            events.iter().filter(|e| e.app == "com.caduceus.desktop.native-helper" && e.service == "Microphone").count();
        assert_eq!(mic_events, 1, "the two mic checks 79ms apart should collapse to one entry");
    }

    #[test]
    fn events_are_sorted_newest_first() {
        let events = parse_tcc_activity(TCC_FIXTURE);
        for pair in events.windows(2) {
            assert!(pair[0].timestamp >= pair[1].timestamp);
        }
    }

    #[test]
    fn empty_log_text_produces_no_events() {
        assert!(parse_tcc_activity("").is_empty());
    }

    // -- file vault: key derivation -----------------------------------------

    #[test]
    fn key_from_passphrase_is_deterministic() {
        let salt = [7u8; VAULT_SALT_LEN];
        let a = key_from_passphrase("correct horse battery staple", &salt).unwrap();
        let b = key_from_passphrase("correct horse battery staple", &salt).unwrap();
        assert_eq!(a, b);
    }

    /// The property the salt exists for: the same passphrase must not produce
    /// the same key twice, or cracking one vault cracks every other.
    #[test]
    fn the_same_passphrase_under_a_different_salt_is_a_different_key() {
        let phrase = "correct horse battery staple";
        let a = key_from_passphrase(phrase, &[1u8; VAULT_SALT_LEN]).unwrap();
        let b = key_from_passphrase(phrase, &[2u8; VAULT_SALT_LEN]).unwrap();
        assert_ne!(a, b);
    }

    /// A salt must never repeat.
    #[test]
    fn every_new_salt_differs() {
        let a = new_salt().unwrap();
        let b = new_salt().unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn different_passphrases_produce_different_keys() {
        let salt = [3u8; VAULT_SALT_LEN];
        let a = key_from_passphrase("correct horse battery staple", &salt).unwrap();
        let b = key_from_passphrase("correct HORSE battery staple", &salt).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn short_passphrases_are_refused() {
        assert!(key_from_passphrase("too short", &[0u8; VAULT_SALT_LEN]).is_err());
    }

    #[test]
    fn a_passphrase_at_exactly_the_minimum_length_is_accepted() {
        let exactly_twelve = "a".repeat(MIN_PASSPHRASE_LEN);
        assert_eq!(exactly_twelve.len(), MIN_PASSPHRASE_LEN);
        assert!(key_from_passphrase(&exactly_twelve, &[0u8; VAULT_SALT_LEN]).is_ok());
    }

    // -- file vault: lock/unlock round trip ----------------------------------
    //
    // Every test file lives under a per-test temp path and is removed at the
    // end, so these do not leave anything behind and do not touch anything
    // outside `std::env::temp_dir()`.

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "caduceus-security-test-{}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn locking_then_unlocking_recovers_the_original_bytes() {
        let original = temp_path("roundtrip.txt");
        let content = b"the quick brown fox jumps over the lazy dog";
        std::fs::write(&original, content).unwrap();

        let vault = lock_file(&original, "a sufficiently long passphrase", false).unwrap();
        assert!(vault.exists());
        assert!(original.exists(), "delete_original was false, the source must survive");

        // Unlocking would refuse because the plaintext path still exists from
        // the setup above, so remove it first to simulate the normal
        // "only the vault remains" case.
        std::fs::remove_file(&original).unwrap();
        let restored = unlock_file(&vault, "a sufficiently long passphrase").unwrap();
        assert_eq!(std::fs::read(&restored).unwrap(), content);

        let _ = std::fs::remove_file(&vault);
        let _ = std::fs::remove_file(&restored);
    }

    #[test]
    fn locking_with_delete_original_removes_the_source() {
        let original = temp_path("delete-original.txt");
        std::fs::write(&original, b"gone after this").unwrap();

        let vault = lock_file(&original, "a sufficiently long passphrase", true).unwrap();
        assert!(!original.exists(), "delete_original was true, the source must be gone");

        let _ = std::fs::remove_file(&vault);
    }

    #[test]
    fn wrong_passphrase_is_refused() {
        let original = temp_path("wrong-passphrase.txt");
        std::fs::write(&original, b"secret contents").unwrap();

        let vault = lock_file(&original, "the correct passphrase here", false).unwrap();
        let err = unlock_file(&vault, "a completely different one").unwrap_err();
        assert!(err.contains("Could not unlock"));

        let _ = std::fs::remove_file(&original);
        let _ = std::fs::remove_file(&vault);
    }

    #[test]
    fn a_file_without_the_vault_header_is_refused() {
        let not_a_vault = temp_path("not-a-vault.bin");
        std::fs::write(&not_a_vault, b"just some random bytes, not ours").unwrap();

        let err = unlock_file(&not_a_vault, "any passphrase length twelve").unwrap_err();
        assert!(err.contains("not a Caduceus vault file"));

        let _ = std::fs::remove_file(&not_a_vault);
    }

    #[test]
    fn locking_refuses_to_overwrite_an_existing_vault_file() {
        let original = temp_path("no-clobber.txt");
        std::fs::write(&original, b"content").unwrap();
        let vault_path = {
            let mut p = original.as_os_str().to_os_string();
            p.push(".vault");
            PathBuf::from(p)
        };
        std::fs::write(&vault_path, b"pretend this is already a vault").unwrap();

        let err = lock_file(&original, "a sufficiently long passphrase", false).unwrap_err();
        assert!(err.contains("already exists"));
        // The pre-existing file must survive untouched.
        assert_eq!(std::fs::read(&vault_path).unwrap(), b"pretend this is already a vault");

        let _ = std::fs::remove_file(&original);
        let _ = std::fs::remove_file(&vault_path);
    }

    #[test]
    fn locking_a_missing_file_is_refused_before_touching_anything() {
        let err = lock_file(Path::new("/definitely/not/a/real/path.txt"), "a sufficiently long passphrase", false)
            .unwrap_err();
        assert!(err.contains("does not exist"));
    }
}
