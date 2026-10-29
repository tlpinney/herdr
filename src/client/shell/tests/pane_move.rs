use super::*;
use crate::api::schema::{Method, PaneDirection};

fn two_pane_state(config: ClientShellConfig, mouse_reporting: bool) -> ClientShellState {
    let mut state = ClientShellState::new(config);
    let mut shell_snapshot = snapshot();
    let mut other = shell_snapshot.panes[0].clone();
    other.pane_id = "pane_2".into();
    other.focused = false;
    shell_snapshot.panes.push(other);
    state.set_snapshot(Box::new(shell_snapshot));

    let mut pane_surface = surface();
    pane_surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(
        &Buffer::empty(Rect::new(0, 0, 80, 19)),
        None,
        &[],
    );
    let rect = |x| SurfaceRect {
        x,
        y: 0,
        width: 40,
        height: 19,
    };
    let first = &mut pane_surface.panes[0];
    first.rect = rect(0);
    first.inner_rect = rect(0);
    first.mouse_reporting = mouse_reporting;
    let mut second = first.clone();
    second.pane_id = "pane_2".into();
    second.rect = rect(40);
    second.inner_rect = rect(40);
    second.focused = false;
    pane_surface.panes.push(second);
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("two pane surface");
    state
}

fn default_two_pane_state() -> ClientShellState {
    two_pane_state(ClientShellConfig::from_config(&Config::default()), false)
}

fn pane_rect(state: &ClientShellState, pane_id: &str) -> Rect {
    state
        .hits
        .panes
        .iter()
        .find(|hit| hit.pane_id == pane_id)
        .map(|hit| hit.rect)
        .expect("pane hit")
}

fn center(rect: Rect) -> (u16, u16) {
    (rect.x + rect.width / 2, rect.y + rect.height / 2)
}

fn mouse(
    state: &mut ClientShellState,
    kind: MouseEventKind,
    point: (u16, u16),
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind,
        column: point.0,
        row: point.1,
        modifiers: KeyModifiers::empty(),
    })])
}

fn hold_past_deadline(state: &mut ClientShellState) -> ClientShellInput {
    let mut outcome = ClientShellInput::default();
    state.tick_pane_long_press(
        std::time::Instant::now() + std::time::Duration::from_secs(1),
        &mut outcome,
    );
    outcome
}

fn pick_up(state: &mut ClientShellState, pane_id: &str) -> ClientShellInput {
    let point = center(pane_rect(state, pane_id));
    mouse(state, MouseEventKind::Down(MouseButton::Left), point);
    let outcome = hold_past_deadline(state);
    assert!(
        state.pane_move_active(),
        "long press should pick the pane up"
    );
    outcome
}

fn endpoint_methods(outcome: &ClientShellInput) -> Vec<&Method> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => Some(&request.method),
            _ => None,
        })
        .collect()
}

fn layout_methods(outcome: &ClientShellInput) -> Vec<&Method> {
    endpoint_methods(outcome)
        .into_iter()
        .filter(|method| matches!(method, Method::PaneSwap(_) | Method::PanePlace(_)))
        .collect()
}

#[test]
fn long_press_release_then_center_click_swaps_panes() {
    let mut state = default_two_pane_state();
    let source = center(pane_rect(&state, "pane_1"));
    let picked = pick_up(&mut state, "pane_1");
    assert!(picked.repaint);
    assert!(
        state.selection.is_none(),
        "pick up clears the press selection"
    );

    let release = mouse(&mut state, MouseEventKind::Up(MouseButton::Left), source);
    assert!(layout_methods(&release).is_empty());
    assert!(
        state.pane_move_active(),
        "releasing in place keeps the pane picked up"
    );

    let target = center(pane_rect(&state, "pane_2"));
    mouse(&mut state, MouseEventKind::Moved, target);
    let drop = mouse(&mut state, MouseEventKind::Down(MouseButton::Left), target);
    assert!(matches!(
        &layout_methods(&drop)[..],
        [Method::PaneSwap(params)]
            if params.source_pane_id.as_deref() == Some("pane_1")
                && params.target_pane_id.as_deref() == Some("pane_2")
    ));
    assert!(!state.pane_move_active());

    let up = mouse(&mut state, MouseEventKind::Up(MouseButton::Left), target);
    assert!(up.actions.is_empty() && up.requests.is_empty());
    assert!(state.selection.is_none());
}

#[test]
fn clicking_a_target_edge_places_the_pane_on_that_side() {
    let mut state = default_two_pane_state();
    let source = center(pane_rect(&state, "pane_1"));
    pick_up(&mut state, "pane_1");
    mouse(&mut state, MouseEventKind::Up(MouseButton::Left), source);

    let target = pane_rect(&state, "pane_2");
    let top_edge = (target.x + target.width / 2, target.y);
    let drop = mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        top_edge,
    );

    assert!(matches!(
        &layout_methods(&drop)[..],
        [Method::PanePlace(params)]
            if params.pane_id == "pane_1"
                && params.target_pane_id == "pane_2"
                && params.side == PaneDirection::Up
                && params.focus
    ));
}

#[test]
fn hold_drag_release_drops_without_a_second_click() {
    let mut state = default_two_pane_state();
    pick_up(&mut state, "pane_1");

    let target = pane_rect(&state, "pane_2");
    let right_edge = (target.right() - 1, target.y + target.height / 2);
    let drag = mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        right_edge,
    );
    assert!(drag.repaint, "hovering a drop zone repaints its preview");
    let release = mouse(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        right_edge,
    );

    assert!(matches!(
        &layout_methods(&release)[..],
        [Method::PanePlace(params)] if params.side == PaneDirection::Right
    ));
    assert!(!state.pane_move_active());
}

#[test]
fn moving_before_the_deadline_keeps_the_press_a_selection() {
    let mut state = default_two_pane_state();
    let (x, y) = center(pane_rect(&state, "pane_1"));
    mouse(&mut state, MouseEventKind::Down(MouseButton::Left), (x, y));
    mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        (x + 3, y),
    );

    hold_past_deadline(&mut state);

    assert!(!state.pane_move_active());
    assert!(state.selection.is_some());
}

#[test]
fn releasing_before_the_deadline_is_an_ordinary_click() {
    let mut state = default_two_pane_state();
    let point = center(pane_rect(&state, "pane_1"));
    mouse(&mut state, MouseEventKind::Down(MouseButton::Left), point);
    mouse(&mut state, MouseEventKind::Up(MouseButton::Left), point);

    hold_past_deadline(&mut state);

    assert!(!state.pane_move_active());
}

#[test]
fn long_press_in_a_mouse_reporting_pane_releases_the_app_press() {
    let mut state = two_pane_state(ClientShellConfig::from_config(&Config::default()), true);
    let point = center(pane_rect(&state, "pane_1"));
    let down = mouse(&mut state, MouseEventKind::Down(MouseButton::Left), point);
    assert!(matches!(
        &down.requests[..],
        [ClientMessage::ClientShellPaneInput { events, .. }]
            if matches!(&events[..], [ClientPaneInputEvent::Mouse {
                kind: crate::protocol::ClientMouseKind::Down(_),
                ..
            }])
    ));

    let picked = hold_past_deadline(&mut state);

    assert!(state.pane_move_active());
    assert!(state.pane_mouse_gesture.is_none());
    assert!(matches!(
        &picked.requests[..],
        [ClientMessage::ClientShellPaneInput { pane_id, events }]
            if pane_id == "pane_1"
                && matches!(&events[..], [ClientPaneInputEvent::Mouse {
                    kind: crate::protocol::ClientMouseKind::Up(
                        crate::protocol::ClientMouseButton::Left
                    ),
                    ..
                }])
    ));
    let release = mouse(&mut state, MouseEventKind::Up(MouseButton::Left), point);
    assert!(
        release.requests.is_empty(),
        "the app must not see a second release"
    );
}

#[test]
fn escape_and_right_click_cancel_a_pane_move() {
    let mut state = default_two_pane_state();
    pick_up(&mut state, "pane_1");
    let cancelled = state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(KeyCode::Esc, KeyModifiers::empty()),
    )]);
    assert!(!state.pane_move_active());
    assert!(cancelled.requests.is_empty(), "esc must not reach the pane");

    pick_up(&mut state, "pane_1");
    let target = center(pane_rect(&state, "pane_2"));
    let right = mouse(&mut state, MouseEventKind::Down(MouseButton::Right), target);
    assert!(!state.pane_move_active());
    assert!(layout_methods(&right).is_empty());
}

#[test]
fn clicking_outside_any_other_pane_cancels_without_a_layout_change() {
    let mut state = default_two_pane_state();
    let source = center(pane_rect(&state, "pane_1"));
    pick_up(&mut state, "pane_1");
    mouse(&mut state, MouseEventKind::Up(MouseButton::Left), source);

    let drop = mouse(&mut state, MouseEventKind::Down(MouseButton::Left), source);

    assert!(layout_methods(&drop).is_empty());
    assert!(!state.pane_move_active());
}

#[test]
fn servers_without_pane_place_only_offer_swap() {
    let mut state = default_two_pane_state();
    state.set_endpoint_methods(Some(vec!["pane.focus".into(), "pane.swap".into()]));
    let source = center(pane_rect(&state, "pane_1"));
    pick_up(&mut state, "pane_1");
    mouse(&mut state, MouseEventKind::Up(MouseButton::Left), source);

    let target = pane_rect(&state, "pane_2");
    let drop = mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        (target.x, target.y + target.height / 2),
    );

    assert!(matches!(&layout_methods(&drop)[..], [Method::PaneSwap(_)]));
}

#[test]
fn long_press_needs_another_pane_and_an_enabled_delay() {
    let mut single = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    single.set_snapshot(Box::new(snapshot()));
    single.set_pane_surface(surface());
    single.compose(106, 20).expect("single pane surface");
    let point = center(single.hits.panes[0].rect);
    mouse(&mut single, MouseEventKind::Down(MouseButton::Left), point);
    hold_past_deadline(&mut single);
    assert!(!single.pane_move_active());

    let mut config = Config::default();
    config.ui.pane_move_long_press_ms = 0;
    let mut disabled = two_pane_state(ClientShellConfig::from_config(&config), false);
    let point = center(pane_rect(&disabled, "pane_1"));
    mouse(
        &mut disabled,
        MouseEventKind::Down(MouseButton::Left),
        point,
    );
    hold_past_deadline(&mut disabled);
    assert!(!disabled.pane_move_active());
    assert!(
        disabled.timer_delay(std::time::Instant::now()) >= std::time::Duration::from_millis(100)
    );
}

#[test]
fn pending_long_press_pulls_the_client_timer_forward() {
    let mut state = default_two_pane_state();
    let point = center(pane_rect(&state, "pane_1"));
    mouse(&mut state, MouseEventKind::Down(MouseButton::Left), point);

    let delay =
        state.timer_delay(std::time::Instant::now() + std::time::Duration::from_millis(350));

    assert!(delay <= std::time::Duration::from_millis(50));
}

#[test]
fn picked_up_pane_is_outlined_and_drop_preview_is_painted() {
    let mut state = default_two_pane_state();
    let source = pane_rect(&state, "pane_1");
    pick_up(&mut state, "pane_1");
    mouse(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        center(source),
    );
    let target = pane_rect(&state, "pane_2");
    mouse(&mut state, MouseEventKind::Moved, center(target));

    let frame = state.compose(106, 20).expect("pane move frame");
    let rows = frame_rows(&frame);

    assert!(rows[usize::from(source.y)].contains("moving"));
    assert!(rows[usize::from(target.y)].contains("swap"));
}
