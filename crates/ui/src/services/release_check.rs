//! A daily check for a newer release of the standalone app.
//!
//! [`UpdateSchedule`] decides when a check is due, and it keeps the last check time and the last
//! good answer in the UI preferences. A check runs on its own thread, so no frame waits for the
//! network, and the answer comes back through a channel that the shell polls.
//!
//! [`GithubReleaseFeed`] asks the GitHub API for the latest release. Its transport reaches only
//! `https://api.github.com`, and the request carries no credential and no account data.
//!
//! An answer only fills the existing update notice. Nothing is downloaded or replaced.

use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

use ronie_core::{
    HttpError, HttpRequest, HttpTransport, OriginPolicy, ReqwestTransport, ResponseKind,
    SettingsStore,
};
use serde_json::{Value, json};
use url::Url;

use super::{links::is_web_url, update::UpdateCheck};

/// The UI preference that turns the check on or off. A missing value means on.
pub const CHECK_PREFERENCE: &str = "t3k.checkForUpdates";
/// The only origin that the release transport can reach.
pub const API_ORIGIN: &str = "https://api.github.com";
/// The address of the latest published release.
pub const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/KonScanner/ronnie-3000/releases/latest";
/// A release page must start with this text, or the answer is not used.
pub const RELEASE_PAGE_PREFIX: &str = "https://github.com/KonScanner/ronnie-3000/releases/";
/// The shortest time between two checks.
pub const CHECK_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;

/// The UI preference that holds the last check time and the last good answer.
const STATE_KEY: &str = "t3k.updateCheck";
/// The connect and header deadline of one check.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The body deadline of one check.
const BODY_DEADLINE: Duration = Duration::from_secs(15);
/// The longest version text that a notice shows.
const MAX_VERSION_LENGTH: usize = 32;

/// Whether the user lets the app check for updates. A missing preference means yes.
#[must_use]
pub fn checks_enabled(preference: Option<bool>) -> bool {
    preference.unwrap_or(true)
}

/// What one finished check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseAnswer {
    /// A release that is newer than this version, as an update payload.
    Newer(Value),
    /// The latest release is not newer than this version.
    Current,
    /// The check failed, or the answer is not usable. The text holds no response body.
    Failed(String),
}

/// One source of release answers.
///
/// [`ReleaseFeed::fetch`] blocks, so only a background thread calls it.
pub trait ReleaseFeed: Send + Sync {
    /// Asks for the latest release, and compares it with the running version.
    fn fetch(&self) -> ReleaseAnswer;
}

/// Writes one diagnostic line.
pub type ReleaseLog = Arc<dyn Fn(&str) + Send + Sync>;

/// The release feed of the GitHub API.
pub struct GithubReleaseFeed<T> {
    transport: T,
    local_version: String,
    log: Option<ReleaseLog>,
}

impl GithubReleaseFeed<ReqwestTransport> {
    /// The production feed for this build. Its transport reaches only [`API_ORIGIN`].
    ///
    /// # Errors
    /// Returns a transport error when the HTTP client cannot be built.
    pub fn github() -> Result<Self, HttpError> {
        let origin = Url::parse(API_ORIGIN).map_err(|_| HttpError::UnsafeOrigin)?;
        let transport = ReqwestTransport::with_timeouts(
            OriginPolicy::new(origin, []),
            CONNECT_TIMEOUT,
            BODY_DEADLINE,
        )?;
        Ok(Self::new(transport, env!("CARGO_PKG_VERSION")))
    }
}

impl<T: HttpTransport + Send + Sync> GithubReleaseFeed<T> {
    /// A feed over `transport` for the running version `local_version`.
    #[must_use]
    pub fn new(transport: T, local_version: impl Into<String>) -> Self {
        Self {
            transport,
            local_version: local_version.into(),
            log: None,
        }
    }

    /// Writes one line to `log` for each failed check.
    #[must_use]
    pub fn with_log(mut self, log: ReleaseLog) -> Self {
        self.log = Some(log);
        self
    }

    fn ask(&self) -> ReleaseAnswer {
        let Some(request) = latest_release_request() else {
            return ReleaseAnswer::Failed("the release address is not valid".to_owned());
        };
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => return ReleaseAnswer::Failed("no runtime could start".to_owned()),
        };
        match runtime.block_on(self.transport.execute(request)) {
            Ok(response) if response.status == 200 => {
                release_answer(&response.body, &self.local_version)
            }
            Ok(response) => {
                ReleaseAnswer::Failed(format!("GitHub answered with status {}", response.status))
            }
            Err(HttpError::ConnectTimeout | HttpError::BodyTimeout) => {
                ReleaseAnswer::Failed("the request timed out".to_owned())
            }
            Err(HttpError::ResponseTooLarge { .. }) => {
                ReleaseAnswer::Failed("the answer was too large".to_owned())
            }
            Err(_) => ReleaseAnswer::Failed("GitHub could not be reached".to_owned()),
        }
    }
}

impl<T: HttpTransport + Send + Sync> ReleaseFeed for GithubReleaseFeed<T> {
    fn fetch(&self) -> ReleaseAnswer {
        let answer = self.ask();
        if let (ReleaseAnswer::Failed(reason), Some(log)) = (&answer, &self.log) {
            log(&format!("The update check failed: {reason}."));
        }
        answer
    }
}

/// The request for the latest release. It has no credential and no account data.
#[must_use]
pub fn latest_release_request() -> Option<HttpRequest> {
    let url = Url::parse(LATEST_RELEASE_URL).ok()?;
    Some(HttpRequest {
        headers: vec![
            (
                "Accept".to_owned(),
                "application/vnd.github+json".to_owned(),
            ),
            (
                "User-Agent".to_owned(),
                format!("Ronnie-3000/{}", env!("CARGO_PKG_VERSION")),
            ),
        ],
        connect_timeout: Some(CONNECT_TIMEOUT),
        body_deadline: Some(BODY_DEADLINE),
        ..HttpRequest::get(url, ResponseKind::Json)
    })
}

/// Reads one answer of the latest-release endpoint.
///
/// A draft, a pre-release, a version that is not plain, or a page outside the project is not
/// used. The result holds no text of the body except the version and the page address.
#[must_use]
pub fn release_answer(body: &[u8], local_version: &str) -> ReleaseAnswer {
    let Ok(release) = serde_json::from_slice::<Value>(body) else {
        return ReleaseAnswer::Failed("the answer is not valid JSON".to_owned());
    };
    if release["draft"] == true || release["prerelease"] == true {
        return ReleaseAnswer::Failed("the latest release is a draft or a pre-release".to_owned());
    }
    let Some(version) = release["tag_name"].as_str().and_then(plain_version) else {
        return ReleaseAnswer::Failed("the release has no usable version".to_owned());
    };
    let Some(page) = release["html_url"]
        .as_str()
        .filter(|url| is_release_page(url))
    else {
        return ReleaseAnswer::Failed("the release page is not on the project address".to_owned());
    };
    if UpdateCheck::compare_versions(version, local_version) <= 0 {
        return ReleaseAnswer::Current;
    }
    ReleaseAnswer::Newer(json!({
        "version": version,
        "message_html": release_message(version),
        "url": page,
    }))
}

/// The tag without a leading `v`, when it is a short plain version.
fn plain_version(tag: &str) -> Option<&str> {
    let version = tag.trim().trim_start_matches(['v', 'V']);
    let plain = !version.is_empty()
        && version.len() <= MAX_VERSION_LENGTH
        && version.starts_with(|character: char| character.is_ascii_digit())
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'));
    plain.then_some(version)
}

fn is_release_page(url: &str) -> bool {
    url.starts_with(RELEASE_PAGE_PREFIX) && url.len() > RELEASE_PAGE_PREFIX.len() && is_web_url(url)
}

/// The plain text of the notice for `version`, the same on each system.
///
/// The notice paints its text, so a user cannot copy a command from it. The release page that
/// "Download update" opens gives the install command as text that the user can copy.
#[must_use]
pub fn release_message(version: &str) -> String {
    format!("Ronnie 3000 {version} is available.")
}

/// The stored state of the check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StoredCheck {
    last_check_ms: Option<i64>,
    payload: Option<Value>,
}

impl StoredCheck {
    fn read(settings: &SettingsStore) -> Self {
        let Ok(properties) = settings.read() else {
            return Self::default();
        };
        let Some(state) = properties
            .get(STATE_KEY)
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
        else {
            return Self::default();
        };
        Self {
            last_check_ms: state["lastCheckMs"].as_i64(),
            // A changed file must not point the notice at another page.
            payload: Some(state["payload"].clone()).filter(|payload| {
                payload["url"].as_str().is_some_and(is_release_page)
                    && payload["version"]
                        .as_str()
                        .and_then(plain_version)
                        .is_some()
            }),
        }
    }

    fn write(&self, settings: &SettingsStore) {
        let state = json!({
            "lastCheckMs": self.last_check_ms,
            "payload": self.payload,
        });
        // The check runs again on the next day, so a failed write loses nothing important.
        let _ = settings.merge_key(STATE_KEY, &state.to_string());
    }
}

/// A check in flight: its answer, and the state to keep after it.
type PendingCheck = mpsc::Receiver<(ReleaseAnswer, StoredCheck)>;

/// When to check, and what the last check found.
pub struct UpdateSchedule {
    feed: Arc<dyn ReleaseFeed>,
    settings: Option<SettingsStore>,
    stored: StoredCheck,
    pending: Option<PendingCheck>,
}

impl UpdateSchedule {
    /// A schedule that reads its stored state from `settings`.
    ///
    /// This reads a file, so a frame must not call it.
    #[must_use]
    pub fn new(feed: Arc<dyn ReleaseFeed>, settings: Option<SettingsStore>) -> Self {
        let stored = settings.as_ref().map(StoredCheck::read).unwrap_or_default();
        Self {
            feed,
            settings,
            stored,
            pending: None,
        }
    }

    /// The last good update payload, which a restart shows again with no request.
    #[must_use]
    pub const fn payload(&self) -> Option<&Value> {
        self.stored.payload.as_ref()
    }

    /// When the last check started, in Unix milliseconds.
    #[must_use]
    pub const fn last_check_ms(&self) -> Option<i64> {
        self.stored.last_check_ms
    }

    /// Whether a check is in flight.
    #[must_use]
    pub const fn checking(&self) -> bool {
        self.pending.is_some()
    }

    /// Whether a check is due at `now_ms`. A clock that moved back also makes a check due.
    #[must_use]
    pub fn due(&self, now_ms: i64) -> bool {
        self.stored
            .last_check_ms
            .is_none_or(|last| now_ms < last || now_ms - last >= CHECK_INTERVAL_MS)
    }

    /// Takes the answer of a finished check, and starts a due check when `may_start` is true.
    ///
    /// This does not block. The stored state is written on the thread of the check, before the
    /// answer arrives here.
    pub fn poll(&mut self, now_ms: i64, may_start: bool) -> Option<ReleaseAnswer> {
        if let Some(pending) = &self.pending {
            return match pending.try_recv() {
                Ok((answer, stored)) => {
                    self.pending = None;
                    self.stored = stored;
                    Some(answer)
                }
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    Some(ReleaseAnswer::Failed("the check stopped".to_owned()))
                }
            };
        }
        if may_start && self.due(now_ms) {
            self.start(now_ms);
        }
        None
    }

    fn start(&mut self, now_ms: i64) {
        // The time counts before the answer, so a failure also waits for the next day.
        self.stored.last_check_ms = Some(now_ms);
        let (sender, receiver) = mpsc::channel();
        let feed = Arc::clone(&self.feed);
        let settings = self.settings.clone();
        let previous = self.stored.clone();
        let spawned = std::thread::Builder::new()
            .name("ronie-update-check".to_owned())
            .spawn(move || {
                let answer = feed.fetch();
                let stored = StoredCheck {
                    last_check_ms: previous.last_check_ms,
                    payload: match &answer {
                        ReleaseAnswer::Newer(payload) => Some(payload.clone()),
                        ReleaseAnswer::Current => None,
                        ReleaseAnswer::Failed(_) => previous.payload,
                    },
                };
                if let Some(settings) = &settings {
                    stored.write(settings);
                }
                let _ = sender.send((answer, stored));
            });
        // With no thread, the next day tries again.
        if spawned.is_ok() {
            self.pending = Some(receiver);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Instant,
    };

    use ronie_core::{HttpResponse, RecordedTransport};

    use super::*;

    const HOUR_MS: i64 = 60 * 60 * 1000;
    const NOW_MS: i64 = 1_800_000_000_000;

    fn release(tag: &str) -> Value {
        json!({
            "tag_name": tag,
            "html_url": format!("{RELEASE_PAGE_PREFIX}tag/{tag}"),
            "draft": false,
            "prerelease": false,
            "body": "Secret release notes that must not reach the notice.",
        })
    }

    fn answer_for(value: &Value, local: &str) -> ReleaseAnswer {
        release_answer(&serde_json::to_vec(value).expect("JSON"), local)
    }

    /// A feed that counts its calls and gives one fixed answer.
    struct CountingFeed {
        calls: AtomicUsize,
        answer: ReleaseAnswer,
    }

    impl CountingFeed {
        fn new(answer: ReleaseAnswer) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                answer,
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl ReleaseFeed for CountingFeed {
        fn fetch(&self) -> ReleaseAnswer {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answer.clone()
        }
    }

    fn newer_payload() -> Value {
        json!({
            "version": "0.2.0",
            "message_html": release_message("0.2.0"),
            "url": format!("{RELEASE_PAGE_PREFIX}tag/v0.2.0"),
        })
    }

    fn settings_with(state: Option<Value>) -> (tempfile::TempDir, SettingsStore) {
        let directory = tempfile::tempdir().expect("settings root");
        let settings = SettingsStore::standalone(directory.path(), "ui-preferences.settings");
        if let Some(state) = state {
            settings
                .merge_key(STATE_KEY, &state.to_string())
                .expect("stored state");
        }
        (directory, settings)
    }

    fn wait_for_answer(schedule: &mut UpdateSchedule, now_ms: i64) -> ReleaseAnswer {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(answer) = schedule.poll(now_ms, true) {
                return answer;
            }
            assert!(Instant::now() < deadline, "no answer");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_newer_release_becomes_a_payload_with_its_page() {
        let ReleaseAnswer::Newer(payload) = answer_for(&release("v0.2.0"), "0.1.0") else {
            panic!("no payload");
        };
        assert_eq!(payload["version"], "0.2.0");
        assert_eq!(payload["url"], format!("{RELEASE_PAGE_PREFIX}tag/v0.2.0"));
        let message = payload["message_html"].as_str().expect("message");
        assert!(
            message.starts_with("Ronnie 3000 0.2.0 is available."),
            "{message}"
        );
        assert!(!message.contains("Secret"), "{message}");
    }

    #[test]
    fn a_leading_v_is_removed_and_a_plain_tag_also_works() {
        for tag in ["v0.3.1", "V0.3.1", "0.3.1"] {
            let ReleaseAnswer::Newer(payload) = answer_for(&release(tag), "0.1.0") else {
                panic!("{tag}: no payload");
            };
            assert_eq!(payload["version"], "0.3.1", "{tag}");
        }
    }

    #[test]
    fn the_same_or_an_older_release_gives_no_payload() {
        for tag in ["v0.1.0", "0.1.0", "v0.0.9"] {
            assert_eq!(
                answer_for(&release(tag), "0.1.0"),
                ReleaseAnswer::Current,
                "{tag}"
            );
        }
    }

    #[test]
    fn a_draft_or_a_pre_release_gives_no_payload() {
        for field in ["draft", "prerelease"] {
            let mut value = release("v9.0.0");
            value[field] = Value::Bool(true);
            assert!(
                matches!(answer_for(&value, "0.1.0"), ReleaseAnswer::Failed(_)),
                "{field}"
            );
        }
    }

    #[test]
    fn a_page_outside_the_project_gives_no_payload() {
        for page in [
            "https://github.com/someone/else/releases/tag/v9.0.0",
            "https://example.invalid/KonScanner/ronnie-3000/releases/tag/v9.0.0",
            "http://github.com/KonScanner/ronnie-3000/releases/tag/v9.0.0",
            "https://github.com/KonScanner/ronnie-3000/releases/",
            "https://github.com/KonScanner/ronnie-3000/releases/tag/v9 0",
        ] {
            let mut value = release("v9.0.0");
            value["html_url"] = Value::String(page.to_owned());
            assert!(
                matches!(answer_for(&value, "0.1.0"), ReleaseAnswer::Failed(_)),
                "{page}"
            );
        }
    }

    #[test]
    fn a_body_that_is_not_json_or_has_no_tag_gives_no_payload() {
        for body in [&b"<html>rate limited</html>"[..], b"", b"{\"tag_name\": 7}"] {
            assert!(matches!(
                release_answer(body, "0.1.0"),
                ReleaseAnswer::Failed(_)
            ));
        }
        let mut value = release("v9.0.0");
        value["tag_name"] = Value::String("v9<script>".to_owned());
        assert!(matches!(
            answer_for(&value, "0.1.0"),
            ReleaseAnswer::Failed(_)
        ));
    }

    #[test]
    fn the_message_is_short_plain_text() {
        let message = release_message("0.2.0");
        assert_eq!(message, "Ronnie 3000 0.2.0 is available.");
        assert!(!message.contains(['<', '>', '&']), "{message}");
        assert!(!message.contains("curl"), "{message}");
    }

    #[test]
    fn the_request_goes_only_to_the_api_with_no_credential() {
        let transport =
            RecordedTransport::new(vec![Ok(HttpResponse::json(200, release("v0.2.0")))]);
        let feed = GithubReleaseFeed::new(transport.clone(), "0.1.0");

        assert!(matches!(feed.fetch(), ReleaseAnswer::Newer(_)));

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.url.as_str(), LATEST_RELEASE_URL);
        assert_eq!(request.url.origin().ascii_serialization(), API_ORIGIN);
        assert!(request.body.is_none());
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        };
        assert_eq!(
            header("Accept").as_deref(),
            Some("application/vnd.github+json")
        );
        let agent = header("User-Agent").expect("user agent");
        assert_eq!(agent, format!("Ronnie-3000/{}", env!("CARGO_PKG_VERSION")));
        assert!(!agent.contains('@'), "{agent}");
        for credential in ["Authorization", "Cookie", "Proxy-Authorization"] {
            assert!(header(credential).is_none(), "{credential}");
        }
        assert!(request.connect_timeout.is_some());
        assert!(request.body_deadline.is_some());
    }

    #[test]
    fn the_production_policy_refuses_every_other_origin() {
        let policy = OriginPolicy::new(Url::parse(API_ORIGIN).expect("origin"), []);
        assert!(
            policy
                .validate(&Url::parse(LATEST_RELEASE_URL).expect("URL"))
                .is_ok()
        );
        for other in [
            "https://github.com/KonScanner/ronnie-3000/releases/latest",
            "https://raw.githubusercontent.com/KonScanner/ronnie-3000/main/install.sh",
            "http://api.github.com/repos/KonScanner/ronnie-3000/releases/latest",
        ] {
            assert!(
                policy.validate(&Url::parse(other).expect("URL")).is_err(),
                "{other}"
            );
        }
    }

    #[test]
    fn a_failure_gives_no_payload_and_writes_one_log_line_with_no_body() {
        for response in [
            Ok(HttpResponse::new(
                403,
                b"{\"message\":\"API rate limit\"}".to_vec(),
            )),
            Ok(HttpResponse::new(200, b"not json at all".to_vec())),
            Err(HttpError::ConnectTimeout),
            Err(HttpError::BodyTimeout),
            Err(HttpError::Transport),
        ] {
            let lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let log: ReleaseLog = {
                let lines = Arc::clone(&lines);
                Arc::new(move |line: &str| {
                    if let Ok(mut lines) = lines.lock() {
                        lines.push(line.to_owned());
                    }
                })
            };
            let feed = GithubReleaseFeed::new(RecordedTransport::new(vec![response]), "0.1.0")
                .with_log(log);

            assert!(matches!(feed.fetch(), ReleaseAnswer::Failed(_)));

            let lines = lines.lock().expect("lines").clone();
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert!(!lines[0].contains("rate limit"), "{lines:?}");
            assert!(!lines[0].contains("not json"), "{lines:?}");
        }
    }

    #[test]
    fn a_first_start_checks_and_stores_the_answer() {
        let (_directory, settings) = settings_with(None);
        let feed = CountingFeed::new(ReleaseAnswer::Newer(newer_payload()));
        let mut schedule = UpdateSchedule::new(feed.clone(), Some(settings.clone()));
        assert!(schedule.due(NOW_MS));

        let answer = wait_for_answer(&mut schedule, NOW_MS);

        assert_eq!(answer, ReleaseAnswer::Newer(newer_payload()));
        assert_eq!(feed.calls(), 1);
        let restarted = UpdateSchedule::new(feed.clone(), Some(settings));
        assert_eq!(restarted.last_check_ms(), Some(NOW_MS));
        assert_eq!(restarted.payload(), Some(&newer_payload()));
    }

    #[test]
    fn no_request_within_a_day_of_the_last_check() {
        let (_directory, settings) = settings_with(Some(json!({
            "lastCheckMs": NOW_MS - HOUR_MS,
            "payload": null,
        })));
        let feed = CountingFeed::new(ReleaseAnswer::Current);
        let mut schedule = UpdateSchedule::new(feed.clone(), Some(settings));

        for now in [NOW_MS, NOW_MS + 22 * HOUR_MS, NOW_MS + 23 * HOUR_MS - 1] {
            assert_eq!(schedule.poll(now, true), None);
            assert!(!schedule.checking(), "{now}");
        }
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(feed.calls(), 0);

        let answer = wait_for_answer(&mut schedule, NOW_MS + 23 * HOUR_MS);
        assert_eq!(answer, ReleaseAnswer::Current);
        assert_eq!(feed.calls(), 1);
    }

    #[test]
    fn a_running_app_checks_again_after_each_day() {
        let feed = CountingFeed::new(ReleaseAnswer::Current);
        let mut schedule = UpdateSchedule::new(feed.clone(), None);

        let _ = wait_for_answer(&mut schedule, NOW_MS);
        assert_eq!(schedule.poll(NOW_MS + CHECK_INTERVAL_MS - 1, true), None);
        assert!(!schedule.checking());
        let _ = wait_for_answer(&mut schedule, NOW_MS + CHECK_INTERVAL_MS);

        assert_eq!(feed.calls(), 2);
    }

    #[test]
    fn a_stored_payload_returns_at_start_with_no_request() {
        let (_directory, settings) = settings_with(Some(json!({
            "lastCheckMs": NOW_MS - HOUR_MS,
            "payload": newer_payload(),
        })));
        let feed = CountingFeed::new(ReleaseAnswer::Current);
        let mut schedule = UpdateSchedule::new(feed.clone(), Some(settings));

        assert_eq!(schedule.payload(), Some(&newer_payload()));
        assert_eq!(schedule.poll(NOW_MS, true), None);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(feed.calls(), 0);
    }

    #[test]
    fn a_stored_payload_for_another_page_is_dropped() {
        let mut payload = newer_payload();
        payload["url"] = Value::String("https://example.invalid/download".to_owned());
        let (_directory, settings) = settings_with(Some(json!({
            "lastCheckMs": NOW_MS,
            "payload": payload,
        })));
        let schedule =
            UpdateSchedule::new(CountingFeed::new(ReleaseAnswer::Current), Some(settings));

        assert_eq!(schedule.payload(), None);
        assert_eq!(schedule.last_check_ms(), Some(NOW_MS));
    }

    #[test]
    fn a_failure_keeps_the_last_good_payload_and_counts_as_a_check() {
        let (_directory, settings) = settings_with(Some(json!({
            "lastCheckMs": NOW_MS - 2 * CHECK_INTERVAL_MS,
            "payload": newer_payload(),
        })));
        let feed = CountingFeed::new(ReleaseAnswer::Failed("offline".to_owned()));
        let mut schedule = UpdateSchedule::new(feed.clone(), Some(settings.clone()));

        let answer = wait_for_answer(&mut schedule, NOW_MS);

        assert!(matches!(answer, ReleaseAnswer::Failed(_)));
        assert_eq!(schedule.payload(), Some(&newer_payload()));
        let restarted = UpdateSchedule::new(feed, Some(settings));
        assert_eq!(restarted.last_check_ms(), Some(NOW_MS));
        assert!(!restarted.due(NOW_MS + HOUR_MS));
    }

    #[test]
    fn a_current_answer_removes_the_stored_payload() {
        let (_directory, settings) = settings_with(Some(json!({
            "lastCheckMs": NOW_MS - 2 * CHECK_INTERVAL_MS,
            "payload": newer_payload(),
        })));
        let feed = CountingFeed::new(ReleaseAnswer::Current);
        let mut schedule = UpdateSchedule::new(feed.clone(), Some(settings.clone()));

        assert_eq!(
            wait_for_answer(&mut schedule, NOW_MS),
            ReleaseAnswer::Current
        );

        assert_eq!(schedule.payload(), None);
        assert_eq!(UpdateSchedule::new(feed, Some(settings)).payload(), None);
    }

    #[test]
    fn a_schedule_that_may_not_start_sends_no_request() {
        let feed = CountingFeed::new(ReleaseAnswer::Newer(newer_payload()));
        let mut schedule = UpdateSchedule::new(feed.clone(), None);

        for hour in 0..72 {
            assert_eq!(schedule.poll(NOW_MS + hour * HOUR_MS, false), None);
        }
        std::thread::sleep(Duration::from_millis(20));

        assert_eq!(feed.calls(), 0);
        assert!(!schedule.checking());
    }

    #[test]
    fn a_missing_preference_means_on() {
        assert!(checks_enabled(None));
        assert!(checks_enabled(Some(true)));
        assert!(!checks_enabled(Some(false)));
    }
}
