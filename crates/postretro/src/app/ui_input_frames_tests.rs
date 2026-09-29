// Splash and Loading frames drop UI input: nothing pressed on them is
// delivered to the first frame that draws UI.
// See: context/lib/input.md §5

use crate::input::{NavIntent, UiIntentPayload};
use crate::startup::BootState;
use crate::startup::lifecycle::tests::test_app;

#[test]
fn input_pressed_on_a_loading_frame_reaches_nothing_on_the_first_running_frame() {
    // A queued confirm and a latched menu toggle from a Loading frame are
    // dropped, never delivered to the first Running frame.
    let mut app = test_app();
    app.boot_state = BootState::Loading;
    assert!(!app.boot_state_accepts_ui_input());
    app.pending_menu_toggle = true;
    {
        let dispatch = &mut app.session.as_mut().unwrap().ui_dispatch;
        dispatch.enqueue_intent(UiIntentPayload::Nav(NavIntent::Confirm));
        dispatch.advance_frame();
        dispatch.enqueue_intent(UiIntentPayload::Nav(NavIntent::Cancel));
    }
    app.drop_ui_input_on_non_ui_frame();

    app.boot_state = BootState::Running;
    assert!(app.boot_state_accepts_ui_input());
    assert!(!app.pending_menu_toggle, "the menu toggle is not latched");
    let dispatch = &mut app.session.as_mut().unwrap().ui_dispatch;
    assert!(dispatch.take_ready().is_empty());
    dispatch.advance_frame();
    assert!(
        dispatch.take_ready().is_empty(),
        "no queued UI input survives"
    );
}
