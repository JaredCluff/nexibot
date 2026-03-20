//! Native desktop control for Computer Use
//!
//! macOS implementation uses CGEvent APIs and AppleScript.
//! Other platforms have stub implementations.

use anyhow::{Context, Result};

// ============================================================================
// macOS Implementation
// ============================================================================

#[cfg(target_os = "macos")]
pub fn screenshot_base64() -> Result<String> {
    use std::process::Command;

    let temp_path =
        std::env::temp_dir().join(format!("nexibot_screenshot_{}.png", uuid::Uuid::new_v4()));
    let temp_str = temp_path.to_str().context("Invalid temp path")?;

    let output = Command::new("screencapture")
        .args(["-x", "-t", "png", temp_str])
        .output()
        .context("Failed to run screencapture")?;

    if !output.status.success() {
        anyhow::bail!(
            "screencapture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let bytes = std::fs::read(&temp_path).context("Failed to read screenshot file")?;
    let _ = std::fs::remove_file(&temp_path);

    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

#[cfg(target_os = "macos")]
pub fn mouse_move(x: i32, y: i32) -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;

    let point = CGPoint::new(x as f64, y as f64);
    let event =
        CGEvent::new_mouse_event(source, CGEventType::MouseMoved, point, CGMouseButton::Left)
            .map_err(|_| anyhow::anyhow!("Failed to create mouse move event"))?;

    event.post(core_graphics::event::CGEventTapLocation::HID);
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn left_click() -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let pos = cursor_position_cg()?;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;

    let down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::LeftMouseDown,
        pos,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create mouse down event"))?;

    let up_source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;

    let up = CGEvent::new_mouse_event(
        up_source,
        CGEventType::LeftMouseUp,
        pos,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create mouse up event"))?;

    down.post(core_graphics::event::CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(50));
    up.post(core_graphics::event::CGEventTapLocation::HID);

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn right_click() -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let pos = cursor_position_cg()?;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;
    let down = CGEvent::new_mouse_event(
        source,
        CGEventType::RightMouseDown,
        pos,
        CGMouseButton::Right,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create right mouse down event"))?;

    let up_source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;
    let up = CGEvent::new_mouse_event(
        up_source,
        CGEventType::RightMouseUp,
        pos,
        CGMouseButton::Right,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create right mouse up event"))?;

    down.post(core_graphics::event::CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(50));
    up.post(core_graphics::event::CGEventTapLocation::HID);

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn double_click() -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let pos = cursor_position_cg()?;

    for click_count in [1u64, 2u64] {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;
        let down =
            CGEvent::new_mouse_event(source, CGEventType::LeftMouseDown, pos, CGMouseButton::Left)
                .map_err(|_| anyhow::anyhow!("Failed to create mouse down event"))?;
        down.set_integer_value_field(
            core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
            click_count as i64,
        );
        down.post(core_graphics::event::CGEventTapLocation::HID);

        std::thread::sleep(std::time::Duration::from_millis(30));

        let up_source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;
        let up = CGEvent::new_mouse_event(
            up_source,
            CGEventType::LeftMouseUp,
            pos,
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow::anyhow!("Failed to create mouse up event"))?;
        up.set_integer_value_field(
            core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
            click_count as i64,
        );
        up.post(core_graphics::event::CGEventTapLocation::HID);

        if click_count == 1 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn type_text(text: &str) -> Result<()> {
    use std::process::Command;

    // Sanitize text for AppleScript to prevent injection.
    // Escape ALL special characters that could break out of the AppleScript string.
    let mut escaped = String::with_capacity(text.len() * 2);
    for ch in text.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if c.is_ascii() && !c.is_ascii_control() => escaped.push(c),
            c if !c.is_ascii() => escaped.push(c), // Allow Unicode through
            _ => {}                                // Skip other control characters
        }
    }

    let script = format!(
        r#"tell application "System Events" to keystroke "{}""#,
        escaped
    );

    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .context("Failed to run osascript for type_text")?;

    if !output.status.success() {
        anyhow::bail!(
            "osascript keystroke failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn key_press(key: &str) -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventFlags};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // Parse key combos like "Return", "ctrl+c", "cmd+shift+s"
    let parts: Vec<&str> = key.split('+').collect();
    let key_name = parts.last().context("Empty key name")?;
    let modifiers = &parts[..parts.len() - 1];

    let keycode =
        key_name_to_keycode(key_name).with_context(|| format!("Unknown key: {}", key_name))?;

    let mut flags = CGEventFlags::empty();
    for modifier in modifiers {
        match modifier.to_lowercase().as_str() {
            "cmd" | "command" | "meta" | "super" => flags |= CGEventFlags::CGEventFlagCommand,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            "alt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            _ => warn_unknown_modifier(modifier),
        }
    }

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;

    let down = CGEvent::new_keyboard_event(source, keycode, true)
        .map_err(|_| anyhow::anyhow!("Failed to create key down event"))?;
    if !flags.is_empty() {
        down.set_flags(flags);
    }

    let up_source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;
    let up = CGEvent::new_keyboard_event(up_source, keycode, false)
        .map_err(|_| anyhow::anyhow!("Failed to create key up event"))?;
    if !flags.is_empty() {
        up.set_flags(flags);
    }

    down.post(core_graphics::event::CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(30));
    up.post(core_graphics::event::CGEventTapLocation::HID);

    Ok(())
}

#[cfg(target_os = "macos")]
fn warn_unknown_modifier(modifier: &str) {
    tracing::warn!("[NATIVE_CONTROL] Unknown modifier: {}", modifier);
}

#[cfg(target_os = "macos")]
pub fn scroll(delta_x: i32, delta_y: i32) -> Result<()> {
    // Use AppleScript for scroll since core-graphics 0.24 doesn't expose CGEventCreateScrollWheelEvent
    // cliclick is another option but AppleScript is universally available
    let _script = r#"tell application "System Events" to scroll area 1 of window 1 of (first application process whose frontmost is true)"#.to_string();

    // Use cliclick-style approach via osascript mouse scroll
    // For precise scrolling, use CGEventCreateScrollWheelEvent via raw FFI
    // SAFETY: CGEventCreateScrollWheelEvent — null source is documented as valid
    // (uses default source). The returned opaque pointer is null-checked before use.
    // CGEventPost — event is non-null (checked above); kCGHIDEventTap=0 is a valid tap.
    // CFRelease — called exactly once on the event pointer, matching one Create call.
    // These are Core Graphics C functions with stable, documented ABIs.
    unsafe {
        extern "C" {
            fn CGEventCreateScrollWheelEvent(
                source: *const std::ffi::c_void,
                units: u32,
                wheel_count: u32,
                wheel1: i32,
                wheel2: i32,
            ) -> *mut std::ffi::c_void;
            fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
            fn CFRelease(cf: *mut std::ffi::c_void);
        }

        let event = CGEventCreateScrollWheelEvent(
            std::ptr::null(),
            1, // kCGScrollEventUnitPixel
            2, // wheel_count
            delta_y,
            delta_x,
        );

        if event.is_null() {
            anyhow::bail!("Failed to create scroll event");
        }

        CGEventPost(0, event); // kCGHIDEventTap = 0
        CFRelease(event);
    }

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn cursor_position() -> Result<(i32, i32)> {
    let pos = cursor_position_cg()?;
    Ok((pos.x as i32, pos.y as i32))
}

#[cfg(target_os = "macos")]
fn cursor_position_cg() -> Result<core_graphics::geometry::CGPoint> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))?;
    let event = CGEvent::new(source).map_err(|_| anyhow::anyhow!("Failed to create CGEvent"))?;
    Ok(event.location())
}

#[cfg(target_os = "macos")]
pub fn check_accessibility_permissions() -> Result<bool> {
    use std::process::Command;

    let output = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to get name of first process"#,
        ])
        .output()
        .context("Failed to check accessibility permissions")?;

    Ok(output.status.success())
}

#[cfg(target_os = "macos")]
pub fn request_accessibility_permissions() -> Result<()> {
    use std::process::Command;

    Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn()
        .context("Failed to open Accessibility preferences")?;

    Ok(())
}

/// Map key names to macOS CGKeyCode values
#[cfg(target_os = "macos")]
fn key_name_to_keycode(name: &str) -> Option<u16> {
    match name.to_lowercase().as_str() {
        "return" | "enter" => Some(0x24),
        "tab" => Some(0x30),
        "space" => Some(0x31),
        "delete" | "backspace" => Some(0x33),
        "escape" | "esc" => Some(0x35),
        "up" | "arrowup" => Some(0x7E),
        "down" | "arrowdown" => Some(0x7D),
        "left" | "arrowleft" => Some(0x7B),
        "right" | "arrowright" => Some(0x7C),
        "home" => Some(0x73),
        "end" => Some(0x77),
        "pageup" | "page_up" => Some(0x74),
        "pagedown" | "page_down" => Some(0x79),
        "forwarddelete" | "forward_delete" => Some(0x75),
        "f1" => Some(0x7A),
        "f2" => Some(0x78),
        "f3" => Some(0x63),
        "f4" => Some(0x76),
        "f5" => Some(0x60),
        "f6" => Some(0x61),
        "f7" => Some(0x62),
        "f8" => Some(0x64),
        "f9" => Some(0x65),
        "f10" => Some(0x6D),
        "f11" => Some(0x67),
        "f12" => Some(0x6F),
        "a" => Some(0x00),
        "b" => Some(0x0B),
        "c" => Some(0x08),
        "d" => Some(0x02),
        "e" => Some(0x0E),
        "f" => Some(0x03),
        "g" => Some(0x05),
        "h" => Some(0x04),
        "i" => Some(0x22),
        "j" => Some(0x26),
        "k" => Some(0x28),
        "l" => Some(0x25),
        "m" => Some(0x2E),
        "n" => Some(0x2D),
        "o" => Some(0x1F),
        "p" => Some(0x23),
        "q" => Some(0x0C),
        "r" => Some(0x0F),
        "s" => Some(0x01),
        "t" => Some(0x11),
        "u" => Some(0x20),
        "v" => Some(0x09),
        "w" => Some(0x0D),
        "x" => Some(0x07),
        "y" => Some(0x10),
        "z" => Some(0x06),
        "0" => Some(0x1D),
        "1" => Some(0x12),
        "2" => Some(0x13),
        "3" => Some(0x14),
        "4" => Some(0x15),
        "5" => Some(0x17),
        "6" => Some(0x16),
        "7" => Some(0x1A),
        "8" => Some(0x1C),
        "9" => Some(0x19),
        _ => None,
    }
}

// ============================================================================
// Windows Implementation
// ============================================================================

#[cfg(target_os = "windows")]
pub fn screenshot_base64() -> Result<String> {
    

    let temp_path =
        std::env::temp_dir().join(format!("nexibot_screenshot_{}.png", uuid::Uuid::new_v4()));
    let temp_str = temp_path.to_string_lossy().to_string();

    // Use PowerShell to capture the screen
    let ps_script = format!(
        r#"Add-Type -AssemblyName System.Windows.Forms; $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds; $bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height); $g = [System.Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size); $bmp.Save('{}'); $g.Dispose(); $bmp.Dispose()"#,
        temp_str.replace('\'', "''")
    );

    let output = crate::platform::hidden_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
        .output()
        .context("Failed to run PowerShell screenshot")?;

    if !output.status.success() {
        anyhow::bail!(
            "PowerShell screenshot failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let bytes = std::fs::read(&temp_path).context("Failed to read screenshot file")?;
    let _ = std::fs::remove_file(&temp_path);

    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

#[cfg(target_os = "windows")]
pub fn mouse_move(x: i32, y: i32) -> Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEINPUT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) } as i32;
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) } as i32;

    // Convert to absolute coordinates (0..65535 range)
    let abs_x = ((x as i64 * 65535) / screen_w as i64) as i32;
    let abs_y = ((y as i64 * 65535) / screen_h as i64) as i32;

    let mut input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: unsafe { std::mem::zeroed() },
    };
    unsafe {
        input.Anonymous.mi = MOUSEINPUT {
            dx: abs_x,
            dy: abs_y,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
            time: 0,
            dwExtraInfo: 0,
        };
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn left_click() -> Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    };

    let inputs = [
        make_mouse_input(MOUSEEVENTF_LEFTDOWN),
        make_mouse_input(MOUSEEVENTF_LEFTUP),
    ];

    unsafe {
        SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn right_click() -> Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    };

    let inputs = [
        make_mouse_input(MOUSEEVENTF_RIGHTDOWN),
        make_mouse_input(MOUSEEVENTF_RIGHTUP),
    ];

    unsafe {
        SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn double_click() -> Result<()> {
    left_click()?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    left_click()?;
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn type_text(text: &str) -> Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    };

    for ch in text.encode_utf16() {
        let inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: unsafe {
                    let mut u: std::mem::MaybeUninit<windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0> = std::mem::MaybeUninit::zeroed();
                    (*u.as_mut_ptr()).ki = KEYBDINPUT {
                        wVk: 0,
                        wScan: ch,
                        dwFlags: KEYEVENTF_UNICODE,
                        time: 0,
                        dwExtraInfo: 0,
                    };
                    u.assume_init()
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: unsafe {
                    let mut u: std::mem::MaybeUninit<windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0> = std::mem::MaybeUninit::zeroed();
                    (*u.as_mut_ptr()).ki = KEYBDINPUT {
                        wVk: 0,
                        wScan: ch,
                        dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    };
                    u.assume_init()
                },
            },
        ];

        unsafe {
            SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn key_press(key: &str) -> Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, KEYEVENTF_KEYUP,
    };

    let parts: Vec<&str> = key.split('+').collect();
    let key_name = parts.last().context("Empty key name")?;
    let modifiers = &parts[..parts.len() - 1];

    let vk = win_key_name_to_vk(key_name)
        .with_context(|| format!("Unknown key: {}", key_name))?;

    // Collect modifier virtual key codes
    let mut mod_vks = Vec::new();
    for modifier in modifiers {
        match modifier.to_lowercase().as_str() {
            "ctrl" | "control" => mod_vks.push(0x11u16), // VK_CONTROL
            "alt" | "option" => mod_vks.push(0x12),      // VK_MENU
            "shift" => mod_vks.push(0x10),               // VK_SHIFT
            "cmd" | "command" | "meta" | "super" | "win" => mod_vks.push(0x5B), // VK_LWIN
            _ => tracing::warn!("[NATIVE_CONTROL] Unknown modifier: {}", modifier),
        }
    }

    let mut inputs = Vec::new();

    // Press modifiers
    for &mvk in &mod_vks {
        inputs.push(make_kb_input(mvk, 0));
    }
    // Press key
    inputs.push(make_kb_input(vk, 0));
    // Release key
    inputs.push(make_kb_input(vk, KEYEVENTF_KEYUP));
    // Release modifiers (reverse order)
    for &mvk in mod_vks.iter().rev() {
        inputs.push(make_kb_input(mvk, KEYEVENTF_KEYUP));
    }

    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn scroll(_delta_x: i32, delta_y: i32) -> Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    };

    let mut input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: unsafe { std::mem::zeroed() },
    };
    unsafe {
        input.Anonymous.mi = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: (delta_y * 120) as u32, // WHEEL_DELTA = 120
            dwFlags: MOUSEEVENTF_WHEEL,
            time: 0,
            dwExtraInfo: 0,
        };
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn cursor_position() -> Result<(i32, i32)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    let ok = unsafe { GetCursorPos(&mut point) };
    if ok == 0 {
        anyhow::bail!("GetCursorPos failed");
    }
    Ok((point.x, point.y))
}

#[cfg(target_os = "windows")]
pub fn check_accessibility_permissions() -> Result<bool> {
    Ok(true) // Windows doesn't require explicit accessibility permissions
}

#[cfg(target_os = "windows")]
pub fn request_accessibility_permissions() -> Result<()> {
    Ok(()) // No-op on Windows
}

// --- Windows helper functions ---

#[cfg(target_os = "windows")]
fn make_mouse_input(flags: u32) -> windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_MOUSE, MOUSEINPUT};

    let mut input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: unsafe { std::mem::zeroed() },
    };
    input.Anonymous.mi = MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: 0,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    input
}

#[cfg(target_os = "windows")]
fn make_kb_input(
    vk: u16,
    flags: u32,
) -> windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_KEYBOARD, KEYBDINPUT};

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: unsafe {
            let mut u: std::mem::MaybeUninit<windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0> = std::mem::MaybeUninit::zeroed();
            (*u.as_mut_ptr()).ki = KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            };
            u.assume_init()
        },
    }
}

/// Map key names to Windows virtual key codes
#[cfg(target_os = "windows")]
fn win_key_name_to_vk(name: &str) -> Option<u16> {
    match name.to_lowercase().as_str() {
        "return" | "enter" => Some(0x0D), // VK_RETURN
        "tab" => Some(0x09),
        "space" => Some(0x20),
        "delete" | "backspace" => Some(0x08), // VK_BACK
        "escape" | "esc" => Some(0x1B),
        "up" | "arrowup" => Some(0x26),
        "down" | "arrowdown" => Some(0x28),
        "left" | "arrowleft" => Some(0x25),
        "right" | "arrowright" => Some(0x27),
        "home" => Some(0x24),
        "end" => Some(0x23),
        "pageup" | "page_up" => Some(0x21),
        "pagedown" | "page_down" => Some(0x22),
        "forwarddelete" | "forward_delete" | "del" => Some(0x2E), // VK_DELETE
        "insert" => Some(0x2D),
        "f1" => Some(0x70),
        "f2" => Some(0x71),
        "f3" => Some(0x72),
        "f4" => Some(0x73),
        "f5" => Some(0x74),
        "f6" => Some(0x75),
        "f7" => Some(0x76),
        "f8" => Some(0x77),
        "f9" => Some(0x78),
        "f10" => Some(0x79),
        "f11" => Some(0x7A),
        "f12" => Some(0x7B),
        // A-Z keys map to 0x41-0x5A
        s if s.len() == 1 && s.chars().next().unwrap().is_ascii_alphabetic() => {
            Some(s.to_uppercase().as_bytes()[0] as u16)
        }
        // 0-9 keys map to 0x30-0x39
        s if s.len() == 1 && s.chars().next().unwrap().is_ascii_digit() => {
            Some(s.as_bytes()[0] as u16)
        }
        _ => None,
    }
}

// ============================================================================
// Linux / Other Stubs
// ============================================================================

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn screenshot_base64() -> Result<String> {
    anyhow::bail!("Screenshot not supported on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn mouse_move(_x: i32, _y: i32) -> Result<()> {
    anyhow::bail!("Mouse control not supported on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn left_click() -> Result<()> {
    anyhow::bail!("Mouse control not supported on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn right_click() -> Result<()> {
    anyhow::bail!("Mouse control not supported on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn double_click() -> Result<()> {
    anyhow::bail!("Mouse control not supported on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn type_text(_text: &str) -> Result<()> {
    anyhow::bail!("Keyboard control not supported on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn key_press(_key: &str) -> Result<()> {
    anyhow::bail!("Keyboard control not supported on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn scroll(_delta_x: i32, _delta_y: i32) -> Result<()> {
    anyhow::bail!("Scroll not supported on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn cursor_position() -> Result<(i32, i32)> {
    anyhow::bail!("Cursor position not supported on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn check_accessibility_permissions() -> Result<bool> {
    Ok(true)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn request_accessibility_permissions() -> Result<()> {
    Ok(())
}
