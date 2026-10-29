//! Long-press pane moving: hold a pane still to pick it up, then click another
//! pane to place it beside (edge) or swap with it (center).

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Widget};

use super::{ClientShellInput, ClientShellMode, ClientShellState, PaneHit};
use crate::api::schema::{Method, PaneDirection, PanePlaceParams, PaneSwapParams};
use crate::protocol::FrameData;

/// Fraction of a pane's width or height, from each edge, that drops beside it.
const EDGE_ZONE: f32 = 0.25;

pub(super) struct ClientPaneLongPress {
    pane_id: String,
    point: (u16, u16),
    deadline: std::time::Instant,
}

pub(super) struct ClientPaneMove {
    pane_id: String,
    /// The pick-up press is still held. Releasing it over a drop zone drops
    /// there, so press-hold-drag-release works as well as press-release-click.
    button_held: bool,
    hover: Option<PaneDropTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PaneDropTarget {
    pane_id: String,
    zone: PaneDropZone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaneDropZone {
    Left,
    Right,
    Up,
    Down,
    Center,
}

/// Which part of `rect` the point falls in: the nearest edge band, or the center.
pub(super) fn drop_zone(rect: Rect, point: (u16, u16)) -> PaneDropZone {
    let fraction = |pos: u16, start: u16, len: u16| {
        (f32::from(pos.saturating_sub(start)) + 0.5) / f32::from(len.max(1))
    };
    let x = fraction(point.0, rect.x, rect.width);
    let y = fraction(point.1, rect.y, rect.height);
    let (zone, distance) = [
        (PaneDropZone::Left, x),
        (PaneDropZone::Right, 1.0 - x),
        (PaneDropZone::Up, y),
        (PaneDropZone::Down, 1.0 - y),
    ]
    .into_iter()
    .fold((PaneDropZone::Center, f32::INFINITY), |best, candidate| {
        if candidate.1 < best.1 {
            candidate
        } else {
            best
        }
    });
    if distance < EDGE_ZONE {
        zone
    } else {
        PaneDropZone::Center
    }
}

/// The part of the target pane the moved pane will occupy.
fn preview_rect(rect: Rect, zone: PaneDropZone) -> Rect {
    let half_width = rect.width / 2;
    let half_height = rect.height / 2;
    match zone {
        PaneDropZone::Left => Rect::new(rect.x, rect.y, half_width, rect.height),
        PaneDropZone::Right => Rect::new(
            rect.x.saturating_add(rect.width - half_width),
            rect.y,
            half_width,
            rect.height,
        ),
        PaneDropZone::Up => Rect::new(rect.x, rect.y, rect.width, half_height),
        PaneDropZone::Down => Rect::new(
            rect.x,
            rect.y.saturating_add(rect.height - half_height),
            rect.width,
            half_height,
        ),
        PaneDropZone::Center => rect,
    }
}

impl ClientShellState {
    pub(super) fn pane_move_active(&self) -> bool {
        self.pane_move.is_some()
    }

    pub(super) fn pane_long_press_deadline(&self) -> Option<std::time::Instant> {
        self.pane_long_press.as_ref().map(|press| press.deadline)
    }

    /// Arm a long press for a left press that already reached normal pane
    /// handling. Nothing changes for the press itself until the timer fires.
    pub(super) fn begin_pane_long_press(&mut self, hit: &PaneHit, mouse: MouseEvent) {
        self.pane_long_press = None;
        let Some(delay) = self.config.pane_move_long_press else {
            return;
        };
        let movable = self.mode == ClientShellMode::Terminal
            && self.overlay.is_none()
            && mouse.modifiers.is_empty()
            && !hit.popup
            && self.hits.panes.len() > 1;
        if !movable {
            return;
        }
        self.pane_long_press = Some(ClientPaneLongPress {
            pane_id: hit.pane_id.clone(),
            point: (mouse.column, mouse.row),
            deadline: std::time::Instant::now() + delay,
        });
    }

    /// Pick the pane up once its press has been held still long enough.
    pub(crate) fn tick_pane_long_press(
        &mut self,
        now: std::time::Instant,
        outcome: &mut ClientShellInput,
    ) {
        if self
            .pane_long_press
            .as_ref()
            .is_none_or(|press| now < press.deadline)
        {
            return;
        }
        let Some(press) = self.pane_long_press.take() else {
            return;
        };
        if self.overlay.is_some()
            || !self
                .hits
                .panes
                .iter()
                .any(|hit| hit.pane_id == press.pane_id)
        {
            return;
        }
        // A mouse-reporting app already saw the press. Release it so the app sees
        // an ordinary click, then herdr owns the rest of the gesture.
        if self
            .pane_mouse_gesture
            .as_ref()
            .is_some_and(|gesture| gesture.hit.pane_id == press.pane_id)
        {
            self.release_pane_mouse_gesture(outcome);
        }
        self.stop_selection_autoscroll();
        self.selection = None;
        self.word_selection_gesture = None;
        self.last_pane_click = None;
        self.pane_move = Some(ClientPaneMove {
            pane_id: press.pane_id,
            button_held: true,
            hover: None,
        });
        outcome.repaint = true;
    }

    pub(super) fn cancel_pane_move(&mut self, outcome: &mut ClientShellInput) {
        if self.pane_move.take().is_some() {
            outcome.repaint = true;
        }
    }

    /// Route mouse input through a pending long press or an active pane move.
    /// Returns true when the event is consumed.
    pub(super) fn handle_pane_move_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let point = (mouse.column, mouse.row);
        self.prune_pane_move(outcome);
        if self.pane_move_consume_up {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => return true,
                MouseEventKind::Up(MouseButton::Left) => {
                    self.pane_move_consume_up = false;
                    return true;
                }
                _ => self.pane_move_consume_up = false,
            }
        }
        if self.pane_move.is_none() {
            let cancels = match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self
                    .pane_long_press
                    .as_ref()
                    .is_some_and(|press| press.point != point),
                MouseEventKind::Down(_) | MouseEventKind::Up(_) => true,
                _ => false,
            };
            if cancels {
                self.pane_long_press = None;
            }
            return false;
        }
        if self.overlay.is_some() {
            self.cancel_pane_move(outcome);
            return false;
        }
        match mouse.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                self.update_pane_move_hover(point, outcome);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let held = self
                    .pane_move
                    .as_ref()
                    .is_some_and(|pane_move| pane_move.button_held);
                if held {
                    self.update_pane_move_hover(point, outcome);
                    let has_target = self
                        .pane_move
                        .as_ref()
                        .is_some_and(|pane_move| pane_move.hover.is_some());
                    if has_target {
                        self.drop_pane_move(outcome);
                    } else if let Some(pane_move) = self.pane_move.as_mut() {
                        pane_move.button_held = false;
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.update_pane_move_hover(point, outcome);
                self.drop_pane_move(outcome);
                self.pane_move_consume_up = true;
            }
            MouseEventKind::Down(_) => self.cancel_pane_move(outcome),
            MouseEventKind::Up(_) | MouseEventKind::Drag(_) => {}
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => return false,
        }
        true
    }

    fn pane_move_target_at(&self, point: (u16, u16)) -> Option<PaneDropTarget> {
        let source = self.pane_move.as_ref()?;
        let hit = self
            .hits
            .panes
            .iter()
            .find(|hit| super::contains(hit.rect, point))?;
        if hit.pane_id == source.pane_id {
            return None;
        }
        let zone = drop_zone(hit.rect, point);
        let place_supported = self.supports_endpoint_method(&Method::PanePlace(PanePlaceParams {
            pane_id: String::new(),
            target_pane_id: String::new(),
            side: PaneDirection::Left,
            ratio: None,
            focus: true,
        }));
        let zone = if place_supported {
            zone
        } else {
            PaneDropZone::Center
        };
        Some(PaneDropTarget {
            pane_id: hit.pane_id.clone(),
            zone,
        })
    }

    fn update_pane_move_hover(&mut self, point: (u16, u16), outcome: &mut ClientShellInput) {
        let hover = self.pane_move_target_at(point);
        if let Some(pane_move) = self.pane_move.as_mut() {
            if pane_move.hover != hover {
                pane_move.hover = hover;
                outcome.repaint = true;
            }
        }
    }

    /// Drop on the hovered target, or cancel when nothing is under the pointer.
    fn drop_pane_move(&mut self, outcome: &mut ClientShellInput) {
        let Some(pane_move) = self.pane_move.take() else {
            return;
        };
        outcome.repaint = true;
        let Some(target) = pane_move.hover else {
            return;
        };
        let side = match target.zone {
            PaneDropZone::Left => PaneDirection::Left,
            PaneDropZone::Right => PaneDirection::Right,
            PaneDropZone::Up => PaneDirection::Up,
            PaneDropZone::Down => PaneDirection::Down,
            PaneDropZone::Center => {
                self.push_endpoint_method(
                    Method::PaneSwap(PaneSwapParams {
                        source_pane_id: Some(pane_move.pane_id),
                        target_pane_id: Some(target.pane_id),
                        ..PaneSwapParams::default()
                    }),
                    outcome,
                );
                return;
            }
        };
        self.push_endpoint_method(
            Method::PanePlace(PanePlaceParams {
                pane_id: pane_move.pane_id,
                target_pane_id: target.pane_id,
                side,
                ratio: None,
                focus: true,
            }),
            outcome,
        );
    }

    /// Drop a move whose pane left the visible surface, e.g. after a tab switch.
    fn prune_pane_move(&mut self, outcome: &mut ClientShellInput) {
        let visible = |pane_id: &str| self.hits.panes.iter().any(|hit| hit.pane_id == pane_id);
        if self
            .pane_move
            .as_ref()
            .is_some_and(|pane_move| !visible(&pane_move.pane_id))
        {
            self.pane_move = None;
            outcome.repaint = true;
        }
        if self
            .pane_long_press
            .as_ref()
            .is_some_and(|press| !visible(&press.pane_id))
        {
            self.pane_long_press = None;
        }
    }

    pub(super) fn render_pane_move(
        &self,
        frame: &mut FrameData,
        occlusion: &mut crate::kitty_graphics::surface::Occlusion,
    ) -> Option<()> {
        let pane_move = self.pane_move.as_ref()?;
        let source = self
            .hits
            .panes
            .iter()
            .find(|hit| hit.pane_id == pane_move.pane_id)?;
        let palette = &self.config.palette;
        let cursor = frame.cursor.clone();
        let mut buffer = frame.to_ratatui_buffer()?;
        let bounds = buffer.area;
        let accent = Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD);

        let source_rect = source.rect.intersection(bounds);
        occlusion.cover(source_rect);
        for y in source_rect.top()..source_rect.bottom() {
            for x in source_rect.left()..source_rect.right() {
                buffer[(x, y)].modifier.insert(Modifier::DIM);
            }
        }
        Block::default()
            .borders(Borders::ALL)
            .border_style(accent)
            .title(" moving · click a pane to place · esc cancels ")
            .title_style(accent)
            .render(source_rect, &mut buffer);

        if let Some(target) = pane_move.hover.as_ref() {
            if let Some(hit) = self
                .hits
                .panes
                .iter()
                .find(|hit| hit.pane_id == target.pane_id)
            {
                let preview = preview_rect(hit.rect, target.zone).intersection(bounds);
                occlusion.cover(preview);
                for y in preview.top()..preview.bottom() {
                    for x in preview.left()..preview.right() {
                        buffer[(x, y)].set_bg(palette.selection_bg);
                    }
                }
                let label = match target.zone {
                    PaneDropZone::Center => " swap ",
                    PaneDropZone::Left => " place left ",
                    PaneDropZone::Right => " place right ",
                    PaneDropZone::Up => " place above ",
                    PaneDropZone::Down => " place below ",
                };
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(accent)
                    .title(label)
                    .title_style(accent)
                    .render(preview, &mut buffer);
            }
        }
        frame.replace_from_ratatui_buffer_preserving_effects(&buffer, cursor);
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_zone_uses_nearest_edge_band_and_center() {
        let rect = Rect::new(10, 5, 40, 20);
        assert_eq!(drop_zone(rect, (10, 15)), PaneDropZone::Left);
        assert_eq!(drop_zone(rect, (49, 15)), PaneDropZone::Right);
        assert_eq!(drop_zone(rect, (30, 5)), PaneDropZone::Up);
        assert_eq!(drop_zone(rect, (30, 24)), PaneDropZone::Down);
        assert_eq!(drop_zone(rect, (30, 15)), PaneDropZone::Center);
        // Just inside the band on each axis.
        assert_eq!(drop_zone(rect, (18, 15)), PaneDropZone::Left);
        assert_eq!(drop_zone(rect, (20, 15)), PaneDropZone::Center);
    }

    #[test]
    fn drop_zone_handles_degenerate_rects() {
        assert_eq!(
            drop_zone(Rect::new(0, 0, 1, 1), (0, 0)),
            PaneDropZone::Center
        );
        assert_eq!(
            drop_zone(Rect::new(0, 0, 0, 0), (0, 0)),
            PaneDropZone::Center
        );
    }

    #[test]
    fn preview_rect_covers_the_dropped_half() {
        let rect = Rect::new(10, 5, 41, 21);
        assert_eq!(
            preview_rect(rect, PaneDropZone::Left),
            Rect::new(10, 5, 20, 21)
        );
        assert_eq!(
            preview_rect(rect, PaneDropZone::Right),
            Rect::new(31, 5, 20, 21)
        );
        assert_eq!(
            preview_rect(rect, PaneDropZone::Up),
            Rect::new(10, 5, 41, 10)
        );
        assert_eq!(
            preview_rect(rect, PaneDropZone::Down),
            Rect::new(10, 16, 41, 10)
        );
        assert_eq!(preview_rect(rect, PaneDropZone::Center), rect);
    }
}
