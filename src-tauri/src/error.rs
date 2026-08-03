use std::fmt;

use serde::ser::SerializeStruct;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppError {
    InvalidItemLimitMb { value: u64, min: u64, max: u64 },
    FavoriteLimitReached { limit: usize },
    ItemNotFound,
    Storage,
    UnsupportedDatabaseVersion,
    InvalidClipboardItem,
    InvalidClipboardImage,
    InvalidMotionScale,
    RepositoryInUse,
    Clipboard,
    Paste,
    ClipboardItemUnavailable,
    CoordinatorUnavailable,
    Platform,
    InvalidPath,
    InvalidDataDirectoryOverride,
    SettingsConsistency,
}

impl AppError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidItemLimitMb { .. } => "invalidItemLimit",
            Self::FavoriteLimitReached { .. } => "favoriteLimitReached",
            Self::ItemNotFound => "itemNotFound",
            Self::Storage => "storageError",
            Self::UnsupportedDatabaseVersion => "unsupportedDatabaseVersion",
            Self::InvalidClipboardItem => "invalidClipboardItem",
            Self::InvalidClipboardImage => "invalidClipboardImage",
            Self::InvalidMotionScale => "invalidMotionScale",
            Self::RepositoryInUse => "repositoryInUse",
            Self::Clipboard => "clipboardError",
            Self::Paste => "pasteError",
            Self::ClipboardItemUnavailable => "clipboardItemUnavailable",
            Self::CoordinatorUnavailable => "coordinatorUnavailable",
            Self::Platform => "platformError",
            Self::InvalidPath => "invalidPath",
            Self::InvalidDataDirectoryOverride => "invalidDataDirectoryOverride",
            Self::SettingsConsistency => "settingsConsistencyError",
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidItemLimitMb { value, min, max } => {
                write!(
                    formatter,
                    "Item limit must be from {min} to {max} MB; received {value} MB"
                )
            }
            Self::FavoriteLimitReached { limit } => {
                write!(
                    formatter,
                    "Favorite limit of {limit} items has been reached"
                )
            }
            Self::ItemNotFound => write!(formatter, "Clipboard item was not found"),
            Self::Storage => write!(formatter, "Clipboard storage operation failed"),
            Self::UnsupportedDatabaseVersion => {
                write!(formatter, "Clipboard database version is not supported")
            }
            Self::InvalidClipboardItem => {
                write!(formatter, "Clipboard item kind does not match its payload")
            }
            Self::InvalidClipboardImage => {
                write!(
                    formatter,
                    "Clipboard image data is invalid or unsafe to decode"
                )
            }
            Self::InvalidMotionScale => write!(formatter, "Motion scale must be finite"),
            Self::RepositoryInUse => {
                write!(formatter, "Clipboard repository is already in use")
            }
            Self::Clipboard => write!(formatter, "Clipboard operation failed"),
            Self::Paste => write!(formatter, "Paste operation failed"),
            Self::ClipboardItemUnavailable => {
                write!(formatter, "Clipboard item is no longer available")
            }
            Self::CoordinatorUnavailable => {
                write!(formatter, "Clipboard coordinator is unavailable")
            }
            Self::Platform => write!(formatter, "Windows platform operation failed"),
            Self::InvalidPath => write!(formatter, "File path is invalid or unavailable"),
            Self::InvalidDataDirectoryOverride => write!(
                formatter,
                "EASY_CLIPBOARD_DATA_DIR must be a non-empty absolute path"
            ),
            Self::SettingsConsistency => {
                write!(
                    formatter,
                    "Settings update could not be rolled back consistently"
                )
            }
        }
    }
}

impl std::error::Error for AppError {}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("AppError", 2)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}

#[cfg(test)]
mod tests {
    use super::AppError;

    #[test]
    fn invalid_item_limit_serializes_to_stable_code_and_message_fields() {
        let error = AppError::InvalidItemLimitMb {
            value: 0,
            min: 1,
            max: 500,
        };

        let json = serde_json::to_value(&error).unwrap();

        assert_eq!(json["code"], "invalidItemLimit");
        assert_eq!(
            json["message"],
            "Item limit must be from 1 to 500 MB; received 0 MB"
        );
        assert_eq!(json.as_object().unwrap().len(), 2);
    }

    #[test]
    fn invalid_item_limit_message_uses_the_carried_range() {
        let error = AppError::InvalidItemLimitMb {
            value: 4,
            min: 2,
            max: 3,
        };

        let json = serde_json::to_value(&error).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "code": "invalidItemLimit",
                "message": "Item limit must be from 2 to 3 MB; received 4 MB"
            })
        );
    }

    #[test]
    fn app_error_implements_std_error() {
        fn assert_std_error<T: std::error::Error>() {}

        assert_std_error::<AppError>();
    }

    #[test]
    fn platform_failure_has_a_stable_sanitized_boundary_shape() {
        let json = serde_json::to_value(AppError::Platform).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "code": "platformError",
                "message": "Windows platform operation failed"
            })
        );
    }
}
