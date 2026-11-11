//! Version notices with dismissed-version snooze memory.

use ronie_core::{PropertiesError, SettingsStore};

use super::links::is_web_url;

const UPDATE_NOTICE_KEY: &str = "t3k.updateNotice";

/// Public update payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    /// Offered version.
    pub version: String,
    /// Safe display message HTML.
    pub message_html: String,
    /// HTTPS or HTTP download page.
    pub url: String,
}

/// Update checker with a quiet best-effort payload policy.
pub struct UpdateCheck {
    local_version: String,
    update: Option<UpdateInfo>,
    notice: Option<UpdateInfo>,
    snooze_until: i64,
    settings: Option<SettingsStore>,
}

impl UpdateCheck {
    /// Creates an in-memory checker for the running version.
    #[must_use]
    pub fn new(local_version: impl Into<String>) -> Self {
        Self::from_settings(local_version, None)
    }

    /// Creates a checker whose snooze is shared through UI preferences.
    #[must_use]
    pub fn with_settings(local_version: impl Into<String>, settings: SettingsStore) -> Self {
        Self::from_settings(local_version, Some(settings))
    }

    fn from_settings(local_version: impl Into<String>, settings: Option<SettingsStore>) -> Self {
        let mut check = Self {
            local_version: local_version.into(),
            update: None,
            notice: None,
            snooze_until: 0,
            settings,
        };
        check.refresh_snooze();
        check
    }

    /// Applies a valid payload, suppressing old or snoozed offers.
    pub fn apply_payload(&mut self, payload: serde_json::Value, now_ms: i64) {
        self.refresh_snooze();
        let Some(info) = parse_payload(&payload) else {
            return;
        };
        if Self::compare_versions(&info.version, &self.local_version) <= 0 {
            self.update = None;
            self.notice = None;
            return;
        }
        self.update = Some(info.clone());
        self.notice = (now_ms >= self.snooze_until).then_some(info);
    }

    /// Dismisses the current notice for a whole number of days.
    ///
    /// # Errors
    /// Returns a preferences error and keeps the notice open if the snooze cannot be saved.
    pub fn remind_later(&mut self, days: i64, now_ms: i64) -> Result<(), PropertiesError> {
        let snooze_until = now_ms.saturating_add(days.max(0).saturating_mul(86_400_000));
        if let Some(settings) = &self.settings {
            settings.merge_key(
                UPDATE_NOTICE_KEY,
                &serde_json::json!({"snoozeUntil": snooze_until}).to_string(),
            )?;
        }
        self.snooze_until = snooze_until;
        self.notice = None;
        Ok(())
    }

    /// Current visible notice.
    #[must_use]
    pub const fn notice(&self) -> Option<&UpdateInfo> {
        self.notice.as_ref()
    }

    /// Latest offered version payload, including a snoozed one.
    #[must_use]
    pub const fn update(&self) -> Option<&UpdateInfo> {
        self.update.as_ref()
    }

    /// Snooze deadline in Unix milliseconds.
    #[must_use]
    pub const fn snooze_until(&self) -> i64 {
        self.snooze_until
    }

    /// Compares dotted numeric versions, ignoring a leading `v` and suffixes.
    #[must_use]
    pub fn compare_versions(left: &str, right: &str) -> i32 {
        let left = segments(left);
        let right = segments(right);
        for index in 0..left.len().max(right.len()) {
            let difference =
                left.get(index).copied().unwrap_or(0) - right.get(index).copied().unwrap_or(0);
            if difference != 0 {
                return difference;
            }
        }
        0
    }

    fn refresh_snooze(&mut self) {
        let Some(settings) = &self.settings else {
            return;
        };
        let Ok(properties) = settings.read() else {
            return;
        };
        let Some(payload) = properties.get(UPDATE_NOTICE_KEY) else {
            self.snooze_until = 0;
            return;
        };
        self.snooze_until = serde_json::from_str::<serde_json::Value>(payload)
            .ok()
            .and_then(|payload| payload.get("snoozeUntil")?.as_i64())
            .unwrap_or(0);
    }
}

fn segments(version: &str) -> Vec<i32> {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|segment| segment.parse::<i32>().unwrap_or(0))
        .collect()
}

fn parse_payload(payload: &serde_json::Value) -> Option<UpdateInfo> {
    let version = payload.get("version")?.as_str()?.to_owned();
    let message_html = payload.get("message_html")?.as_str()?.to_owned();
    let url = payload.get("url")?.as_str()?.to_owned();
    if !is_web_url(&url) {
        return None;
    }
    Some(UpdateInfo {
        version,
        message_html,
        url,
    })
}
