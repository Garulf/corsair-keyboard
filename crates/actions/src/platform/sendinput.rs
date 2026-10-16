use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, MOUSEINPUT, SendInput,
};

use super::windows_inputs::{WinInput, win_inputs};
use crate::sink::{InputSink, SinkError, SinkEvent};

pub struct SendInputSink;

impl InputSink for SendInputSink {
    fn send(&mut self, event: &SinkEvent) -> Result<(), SinkError> {
        let inputs: Vec<INPUT> = win_inputs(event).into_iter().map(to_input).collect();
        if inputs.is_empty() {
            return Ok(());
        }
        // SAFETY: `inputs` is a live, correctly sized array of INPUT records for the duration of the call.
        let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() {
            Ok(())
        } else {
            Err(SinkError(format!("SendInput accepted {sent} of {} events", inputs.len())))
        }
    }

    fn needs_typematic(&self) -> bool {
        true
    }
}

fn to_input(input: WinInput) -> INPUT {
    match input {
        WinInput::Key { vk, scan, flags } => INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
        },
        WinInput::Mouse { flags, data } => INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: data as _, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        },
    }
}
