use x11rb::{
    CURRENT_TIME,
    connection::Connection,
    protocol::xproto::{ConnectionExt, InputFocus, MapState},
};

/// Focus the explicitly opened panel once it is mapped. Call on a blocking
/// worker, because querying the X server entails a round trip. Wayland windows
/// must never be passed here; their compositor owns focus and activation.
pub fn focus_x11_panel(window_id: u32) -> Result<(), String> {
    let (connection, _) = x11rb::connect(None).map_err(|error| error.to_string())?;
    let attributes = connection
        .get_window_attributes(window_id)
        .map_err(|error| error.to_string())?
        .reply()
        .map_err(|error| error.to_string())?;
    if attributes.map_state != MapState::VIEWABLE {
        return Err(
            "The tray panel is not mapped; keeping the window manager activation request".into(),
        );
    }
    connection
        .set_input_focus(InputFocus::PARENT, window_id, CURRENT_TIME)
        .map_err(|error| error.to_string())?
        .check()
        .map_err(|error| error.to_string())?;
    connection.flush().map_err(|error| error.to_string())
}
