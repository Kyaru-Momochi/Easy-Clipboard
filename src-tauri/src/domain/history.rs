use super::{ClipboardItem, ItemId};
use crate::error::AppError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryPolicy {
    normal_limit: usize,
    favorite_limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpsertDecision {
    Insert { evict: Option<ItemId> },
    Touch { existing: ItemId },
}

impl HistoryPolicy {
    pub fn new(normal_limit: usize, favorite_limit: usize) -> Self {
        Self {
            normal_limit,
            favorite_limit,
        }
    }

    pub fn decide_upsert(
        &self,
        incoming_fingerprint: &str,
        current: &[ClipboardItem],
    ) -> UpsertDecision {
        if let Some(existing) = current
            .iter()
            .find(|item| item.fingerprint == incoming_fingerprint)
        {
            return UpsertDecision::Touch {
                existing: existing.id.clone(),
            };
        }

        let normal_count = current.iter().filter(|item| !item.is_favorite).count();
        let evict = if normal_count < self.normal_limit {
            None
        } else {
            current
                .iter()
                .filter(|item| !item.is_favorite)
                .min_by_key(|item| (item.updated_at_ms, item.created_at_ms, item.id.0.as_str()))
                .map(|item| item.id.clone())
        };

        UpsertDecision::Insert { evict }
    }

    pub fn can_favorite(&self, current: &[ClipboardItem]) -> Result<(), AppError> {
        let favorite_count = current.iter().filter(|item| item.is_favorite).count();

        if favorite_count < self.favorite_limit {
            Ok(())
        } else {
            Err(AppError::FavoriteLimitReached {
                limit: self.favorite_limit,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HistoryPolicy, UpsertDecision};
    use crate::{
        domain::{ClipboardItem, ClipboardKind, ClipboardPayload, ItemId},
        error::AppError,
    };

    fn item(
        id: &str,
        fingerprint: &str,
        is_favorite: bool,
        created_at_ms: i64,
        updated_at_ms: i64,
    ) -> ClipboardItem {
        ClipboardItem {
            id: ItemId(id.into()),
            kind: ClipboardKind::Text,
            payload: ClipboardPayload::Text {
                plain: format!("payload-{id}"),
                html: None,
                rtf: None,
            },
            fingerprint: fingerprint.into(),
            preview: format!("preview-{id}"),
            byte_size: 1,
            is_favorite,
            created_at_ms,
            updated_at_ms,
        }
    }

    #[test]
    fn duplicate_content_moves_to_front_without_growing_history() {
        let policy = HistoryPolicy::new(100, 20);
        let current = vec![
            item("item-1", "first", false, 100, 200),
            item("item-2", "matching", true, 300, 400),
        ];

        let decision = policy.decide_upsert("matching", &current);

        assert_eq!(
            decision,
            UpsertDecision::Touch {
                existing: ItemId("item-2".into())
            }
        );
    }

    #[test]
    fn adding_item_101_evicts_oldest_non_favorite() {
        let policy = HistoryPolicy::new(100, 20);
        let current = (0..100)
            .map(|index| {
                item(
                    &format!("item-{index}"),
                    &format!("fingerprint-{index}"),
                    false,
                    index,
                    index,
                )
            })
            .collect::<Vec<_>>();

        let decision = policy.decide_upsert("new-fingerprint", &current);

        assert_eq!(
            decision,
            UpsertDecision::Insert {
                evict: Some(ItemId("item-0".into()))
            }
        );
    }

    #[test]
    fn favorites_are_kept_outside_normal_rotation() {
        let policy = HistoryPolicy::new(100, 20);
        let mut current = vec![item("favorite", "favorite", true, 0, 0)];
        current.extend((0..100).map(|index| {
            item(
                &format!("normal-{index}"),
                &format!("normal-fingerprint-{index}"),
                false,
                index + 100,
                index + 100,
            )
        }));

        let decision = policy.decide_upsert("new-fingerprint", &current);

        assert_eq!(
            decision,
            UpsertDecision::Insert {
                evict: Some(ItemId("normal-0".into()))
            }
        );
    }

    #[test]
    fn favorite_21_is_rejected() {
        let policy = HistoryPolicy::new(100, 20);
        let current = (0..20)
            .map(|index| {
                item(
                    &format!("favorite-{index}"),
                    &format!("fingerprint-{index}"),
                    true,
                    index,
                    index,
                )
            })
            .collect::<Vec<_>>();

        let error = policy.can_favorite(&current).unwrap_err();
        let json = serde_json::to_value(&error).unwrap();

        assert_eq!(error, AppError::FavoriteLimitReached { limit: 20 });
        assert_eq!(
            json,
            serde_json::json!({
                "code": "favoriteLimitReached",
                "message": "Favorite limit of 20 items has been reached"
            })
        );
    }

    #[test]
    fn below_limits_insert_without_eviction() {
        let policy = HistoryPolicy::new(3, 2);
        let current = vec![
            item("normal", "normal", false, 100, 100),
            item("favorite", "favorite", true, 50, 50),
        ];

        assert_eq!(
            policy.decide_upsert("new-fingerprint", &current),
            UpsertDecision::Insert { evict: None }
        );
        assert_eq!(policy.can_favorite(&current), Ok(()));
    }

    #[test]
    fn deterministic_oldest_tie_breaker() {
        let policy = HistoryPolicy::new(3, 20);
        let current = vec![
            item("item-b", "b", false, 100, 1_000),
            item("item-c", "c", false, 50, 1_000),
            item("item-a", "a", false, 50, 1_000),
        ];

        assert_eq!(
            policy.decide_upsert("new-fingerprint", &current),
            UpsertDecision::Insert {
                evict: Some(ItemId("item-a".into()))
            }
        );
    }

    #[test]
    fn zero_limits_do_not_panic() {
        let policy = HistoryPolicy::new(0, 0);

        assert_eq!(
            policy.decide_upsert("new-fingerprint", &[]),
            UpsertDecision::Insert { evict: None }
        );

        let error = policy.can_favorite(&[]).unwrap_err();
        assert_eq!(error, AppError::FavoriteLimitReached { limit: 0 });
    }
}
