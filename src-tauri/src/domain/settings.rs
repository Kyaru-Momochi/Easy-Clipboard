use crate::{domain::ThemeMode, error::AppError};

const BYTES_PER_MEGABYTE: u64 = 1024 * 1024;
const MIN_ITEM_LIMIT_MB: u64 = 1;
const MAX_ITEM_LIMIT_MB: u64 = 500;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub history_limit: usize,
    pub favorite_limit: usize,
    pub max_item_bytes: u64,
    pub hotkey: String,
    pub theme: ThemeMode,
    pub motion_scale: f32,
    pub autostart: bool,
    pub clear_on_exit: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            history_limit: 100,
            favorite_limit: 20,
            max_item_bytes: 50 * BYTES_PER_MEGABYTE,
            hotkey: "Ctrl+Shift+V".into(),
            theme: ThemeMode::System,
            motion_scale: 1.0,
            autostart: false,
            clear_on_exit: false,
        }
    }
}

impl AppSettings {
    pub fn with_limit_mb(mut self, limit_mb: u64) -> Result<Self, AppError> {
        if !(MIN_ITEM_LIMIT_MB..=MAX_ITEM_LIMIT_MB).contains(&limit_mb) {
            return Err(AppError::InvalidItemLimitMb {
                value: limit_mb,
                min: MIN_ITEM_LIMIT_MB,
                max: MAX_ITEM_LIMIT_MB,
            });
        }

        self.max_item_bytes = limit_mb * BYTES_PER_MEGABYTE;
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{AppSettings, ThemeMode};

    #[test]
    fn settings_default_to_confirmed_product_values() {
        let settings = AppSettings::default();

        assert_eq!(settings.history_limit, 100);
        assert_eq!(settings.favorite_limit, 20);
        assert_eq!(settings.max_item_bytes, 50 * 1024 * 1024);
        assert_eq!(settings.hotkey, "Ctrl+Shift+V");
        assert_eq!(settings.theme, ThemeMode::System);
        assert_eq!(settings.motion_scale, 1.0);
        assert!(!settings.autostart);
        assert!(!settings.clear_on_exit);
    }

    #[test]
    fn item_limit_accepts_one_to_five_hundred_megabytes() {
        assert!(AppSettings::default().with_limit_mb(1).is_ok());
        assert!(AppSettings::default().with_limit_mb(500).is_ok());

        let below_range = AppSettings::default().with_limit_mb(0).unwrap_err();
        assert_eq!(below_range.code(), "invalidItemLimit");
        assert!(below_range.to_string().contains("1 to 500 MB"));

        let above_range = AppSettings::default().with_limit_mb(501).unwrap_err();
        assert_eq!(above_range.code(), "invalidItemLimit");
        assert!(above_range.to_string().contains("1 to 500 MB"));
    }

    #[test]
    fn item_limit_converts_megabytes_to_exact_bytes() {
        let original = AppSettings::default();

        let updated = original.clone().with_limit_mb(37).unwrap();

        assert_eq!(updated.max_item_bytes, 37 * 1024 * 1024);
        assert_eq!(original.max_item_bytes, 50 * 1024 * 1024);
    }
}
