use std::{
    sync::{Mutex, mpsc::Receiver},
    thread::{self, JoinHandle},
};

use serde::Serialize;
use tauri::{Emitter, WebviewWindow};

use crate::error::AppError;

use super::{KeyAction, OverlayController, OverlayPlacement, Size, VisibleKeyboardRouter};

pub const SEARCH_INPUT_EVENT: &str = "search-input";
pub const SELECTION_MOVE_EVENT: &str = "selection-move";
pub const SELECTION_PASTE_EVENT: &str = "selection-paste";
pub const OVERLAY_HIDE_EVENT: &str = "overlay-hide";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum SearchInputPayload {
    Insert { text: String },
    DeleteBackward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionMovePayload {
    pub delta: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayEvent {
    SearchInput(SearchInputPayload),
    SelectionMove(SelectionMovePayload),
    SelectionPaste,
    Hide,
}

pub fn event_for_action(action: KeyAction) -> OverlayEvent {
    match action {
        KeyAction::SearchText(text) => {
            OverlayEvent::SearchInput(SearchInputPayload::Insert { text })
        }
        KeyAction::DeleteBackward => OverlayEvent::SearchInput(SearchInputPayload::DeleteBackward),
        KeyAction::Move(delta) => OverlayEvent::SelectionMove(SelectionMovePayload { delta }),
        KeyAction::Paste => OverlayEvent::SelectionPaste,
        KeyAction::Hide => OverlayEvent::Hide,
    }
}

/// Tauri-managed runtime for the focus-safe overlay and its visible-only keyboard event bridge.
pub struct OverlayRuntime {
    overlay: OverlayController,
    keyboard: VisibleKeyboardRouter,
    window: WebviewWindow,
    bridge: Mutex<Option<JoinHandle<()>>>,
    operation: Mutex<()>,
}

impl OverlayRuntime {
    pub fn new(overlay: OverlayController, window: WebviewWindow) -> Self {
        Self {
            overlay,
            keyboard: VisibleKeyboardRouter::default(),
            window,
            bridge: Mutex::new(None),
            operation: Mutex::new(()),
        }
    }

    pub fn show(
        &self,
        remembered_foreground: isize,
        requested: Size,
    ) -> Result<OverlayPlacement, AppError> {
        let _operation = self.operation.lock().map_err(|_| AppError::Platform)?;
        let mut bridge = self.bridge.lock().map_err(|_| AppError::Platform)?;
        if bridge.is_some() || self.keyboard.is_visible() {
            return Err(AppError::Platform);
        }

        let placement = self
            .overlay
            .show_centered(remembered_foreground, requested)?;
        let receiver = match self.keyboard.show() {
            Ok(receiver) => receiver,
            Err(error) => {
                let _ = self.overlay.hide();
                return Err(error);
            }
        };
        let window = self.window.clone();
        match thread::Builder::new()
            .name("easy-clipboard-overlay-events".into())
            .spawn(move || bridge_events(&window, receiver))
        {
            Ok(thread) => {
                *bridge = Some(thread);
                Ok(placement)
            }
            Err(_) => {
                let _ = self.keyboard.hide();
                let _ = self.overlay.hide();
                Err(AppError::Platform)
            }
        }
    }

    pub fn hide(&self) -> Result<(), AppError> {
        let _operation = self.operation.lock().map_err(|_| AppError::Platform)?;
        let mut bridge = self.bridge.lock().map_err(|_| AppError::Platform)?;
        stop_then_join_bridge(
            &mut bridge,
            || self.keyboard.hide(),
            |thread| thread.join().map_err(|_| AppError::Platform),
        )?;
        self.overlay.hide()
    }

    pub fn is_visible(&self) -> bool {
        self.keyboard.is_visible()
    }
}

impl Drop for OverlayRuntime {
    fn drop(&mut self) {
        if let Ok(bridge) = self.bridge.get_mut() {
            let _ = stop_then_join_bridge(
                bridge,
                || self.keyboard.hide(),
                |thread| thread.join().map_err(|_| AppError::Platform),
            );
        }
        let _ = self.overlay.hide();
    }
}

fn stop_then_join_bridge<T>(
    bridge: &mut Option<T>,
    stop: impl FnOnce() -> Result<(), AppError>,
    join: impl FnOnce(T) -> Result<(), AppError>,
) -> Result<(), AppError> {
    stop()?;
    if let Some(worker) = bridge.take() {
        join(worker)?;
    }
    Ok(())
}

fn bridge_events(window: &WebviewWindow, receiver: Receiver<KeyAction>) {
    for action in receiver {
        if emit_event(window, event_for_action(action)).is_err() {
            break;
        }
    }
}

fn emit_event(window: &WebviewWindow, event: OverlayEvent) -> tauri::Result<()> {
    match event {
        OverlayEvent::SearchInput(payload) => window.emit(SEARCH_INPUT_EVENT, payload),
        OverlayEvent::SelectionMove(payload) => window.emit(SELECTION_MOVE_EVENT, payload),
        OverlayEvent::SelectionPaste => window.emit(SELECTION_PASTE_EVENT, ()),
        OverlayEvent::Hide => window.emit(OVERLAY_HIDE_EVENT, ()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OVERLAY_HIDE_EVENT, OverlayEvent, SEARCH_INPUT_EVENT, SELECTION_MOVE_EVENT,
        SELECTION_PASTE_EVENT, SearchInputPayload, SelectionMovePayload, event_for_action,
        stop_then_join_bridge,
    };
    use crate::platform::KeyAction;

    #[test]
    fn task11_event_names_are_exact_and_centralized() {
        assert_eq!(SEARCH_INPUT_EVENT, "search-input");
        assert_eq!(SELECTION_MOVE_EVENT, "selection-move");
        assert_eq!(SELECTION_PASTE_EVENT, "selection-paste");
        assert_eq!(OVERLAY_HIDE_EVENT, "overlay-hide");
    }

    #[test]
    fn search_actions_share_search_input_event_with_stable_payloads() {
        assert_eq!(
            event_for_action(KeyAction::SearchText("Ab".into())),
            OverlayEvent::SearchInput(SearchInputPayload::Insert { text: "Ab".into() })
        );
        assert_eq!(
            event_for_action(KeyAction::DeleteBackward),
            OverlayEvent::SearchInput(SearchInputPayload::DeleteBackward)
        );
    }

    #[test]
    fn selection_paste_and_hide_actions_map_to_task11_events() {
        assert_eq!(
            event_for_action(KeyAction::Move(-1)),
            OverlayEvent::SelectionMove(SelectionMovePayload { delta: -1 })
        );
        assert_eq!(
            event_for_action(KeyAction::Paste),
            OverlayEvent::SelectionPaste
        );
        assert_eq!(event_for_action(KeyAction::Hide), OverlayEvent::Hide);
    }

    #[test]
    fn event_payloads_serialize_to_stable_camel_case_shapes() {
        assert_eq!(
            serde_json::to_value(SearchInputPayload::Insert { text: "你".into() }).unwrap(),
            serde_json::json!({"action": "insert", "text": "你"})
        );
        assert_eq!(
            serde_json::to_value(SearchInputPayload::DeleteBackward).unwrap(),
            serde_json::json!({"action": "deleteBackward"})
        );
        assert_eq!(
            serde_json::to_value(SelectionMovePayload { delta: 1 }).unwrap(),
            serde_json::json!({"delta": 1})
        );
    }

    #[test]
    fn stop_failure_keeps_bridge_owned_and_never_calls_join() {
        let mut bridge = Some("bridge");
        let joined = std::cell::Cell::new(false);

        let result = stop_then_join_bridge(
            &mut bridge,
            || Err(crate::error::AppError::Platform),
            |_| {
                joined.set(true);
                Ok(())
            },
        );

        assert_eq!(result, Err(crate::error::AppError::Platform));
        assert_eq!(bridge, Some("bridge"));
        assert!(!joined.get());
    }

    #[test]
    fn successful_stop_takes_and_joins_bridge() {
        let mut bridge = Some("bridge");
        let joined = std::cell::Cell::new(false);

        stop_then_join_bridge(
            &mut bridge,
            || Ok(()),
            |value| {
                assert_eq!(value, "bridge");
                joined.set(true);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(bridge, None);
        assert!(joined.get());
    }
}
