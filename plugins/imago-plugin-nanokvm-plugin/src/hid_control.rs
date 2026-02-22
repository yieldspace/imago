use std::{
    collections::HashMap, fs, io::Write, path::Path, process::Command, thread, time::Duration,
};

use wasmtime::component::ResourceTable;

use crate::{
    common::{ensure_nanokvm_environment, read_file_trimmed, unsupported_error},
    constants::{
        HID_DEVICE_ABSOLUTE_MOUSE, HID_DEVICE_KEYBOARD, HID_DEVICE_RELATIVE_MOUSE,
        HID_MODE_HID_ONLY_SCRIPT, HID_MODE_NORMAL_SCRIPT, HID_MODE_TARGET_SCRIPT,
        USB_MODE_FLAG_PATH,
    },
    device_status::parse_usb_mode,
    types::{
        AbsoluteMouseEvent, HidMode, KeyboardEvent, KeyboardLayout, PasteKey, RelativeMouseEvent,
        TouchEvent, UsbMode,
    },
};

fn parse_hid_mode_from_usb_mode(raw: &str) -> Result<HidMode, String> {
    match parse_usb_mode(raw)? {
        UsbMode::Normal => Ok(HidMode::HidAndMouse),
        UsbMode::HidOnly => Ok(HidMode::Hid),
    }
}

pub(crate) fn hid_mode_script_source(mode: HidMode) -> Result<&'static str, String> {
    match mode {
        HidMode::Hid => Ok(HID_MODE_HID_ONLY_SCRIPT),
        HidMode::HidAndMouse => Ok(HID_MODE_NORMAL_SCRIPT),
        HidMode::HidAndTouchpad => {
            Err("unsupported hid mode: hid-and-touchpad is not implemented".to_string())
        }
        HidMode::HidAndAbsoluteMouse => {
            Err("unsupported hid mode: hid-and-absolute-mouse is not implemented".to_string())
        }
    }
}

fn write_hid_report(path: &str, report: &[u8]) -> Result<(), String> {
    if !Path::new(path).exists() {
        return Err(unsupported_error(format!("missing {path}")));
    }

    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|err| format!("failed to open {path}: {err}"))?;
    file.write_all(report)
        .and_then(|_| file.flush())
        .map_err(|err| format!("failed to write {path}: {err}"))
}

fn clamp_to_i8(value: i16) -> i8 {
    value.clamp(-127, 127) as i8
}

pub(crate) fn build_keyboard_report(event: &KeyboardEvent) -> Result<[u8; 8], String> {
    if event.keycodes.len() > 6 {
        return Err("keyboard-event.keycodes must contain at most 6 keys".to_string());
    }

    let mut report = [0u8; 8];
    report[0] = event.modifiers;
    for (index, keycode) in event.keycodes.iter().enumerate() {
        report[index + 2] = *keycode;
    }
    Ok(report)
}

pub(crate) fn build_relative_mouse_report(event: &RelativeMouseEvent) -> [u8; 4] {
    [
        event.buttons,
        clamp_to_i8(event.dx) as u8,
        clamp_to_i8(event.dy) as u8,
        clamp_to_i8(event.wheel) as u8,
    ]
}

pub(crate) fn build_absolute_mouse_report(event: &AbsoluteMouseEvent) -> [u8; 6] {
    [
        event.buttons,
        (event.x & 0x00ff) as u8,
        ((event.x >> 8) & 0x00ff) as u8,
        (event.y & 0x00ff) as u8,
        ((event.y >> 8) & 0x00ff) as u8,
        clamp_to_i8(event.wheel) as u8,
    ]
}

pub(crate) fn build_touch_report(event: &TouchEvent) -> [u8; 6] {
    [
        u8::from(event.pressed),
        (event.x & 0x00ff) as u8,
        ((event.x >> 8) & 0x00ff) as u8,
        (event.y & 0x00ff) as u8,
        ((event.y >> 8) & 0x00ff) as u8,
        0,
    ]
}

fn paste_char_map(layout: KeyboardLayout) -> HashMap<char, PasteKey> {
    let mut map = HashMap::from([
        (
            'a',
            PasteKey {
                modifiers: 0,
                code: 4,
            },
        ),
        (
            'b',
            PasteKey {
                modifiers: 0,
                code: 5,
            },
        ),
        (
            'c',
            PasteKey {
                modifiers: 0,
                code: 6,
            },
        ),
        (
            'd',
            PasteKey {
                modifiers: 0,
                code: 7,
            },
        ),
        (
            'e',
            PasteKey {
                modifiers: 0,
                code: 8,
            },
        ),
        (
            'f',
            PasteKey {
                modifiers: 0,
                code: 9,
            },
        ),
        (
            'g',
            PasteKey {
                modifiers: 0,
                code: 10,
            },
        ),
        (
            'h',
            PasteKey {
                modifiers: 0,
                code: 11,
            },
        ),
        (
            'i',
            PasteKey {
                modifiers: 0,
                code: 12,
            },
        ),
        (
            'j',
            PasteKey {
                modifiers: 0,
                code: 13,
            },
        ),
        (
            'k',
            PasteKey {
                modifiers: 0,
                code: 14,
            },
        ),
        (
            'l',
            PasteKey {
                modifiers: 0,
                code: 15,
            },
        ),
        (
            'm',
            PasteKey {
                modifiers: 0,
                code: 16,
            },
        ),
        (
            'n',
            PasteKey {
                modifiers: 0,
                code: 17,
            },
        ),
        (
            'o',
            PasteKey {
                modifiers: 0,
                code: 18,
            },
        ),
        (
            'p',
            PasteKey {
                modifiers: 0,
                code: 19,
            },
        ),
        (
            'q',
            PasteKey {
                modifiers: 0,
                code: 20,
            },
        ),
        (
            'r',
            PasteKey {
                modifiers: 0,
                code: 21,
            },
        ),
        (
            's',
            PasteKey {
                modifiers: 0,
                code: 22,
            },
        ),
        (
            't',
            PasteKey {
                modifiers: 0,
                code: 23,
            },
        ),
        (
            'u',
            PasteKey {
                modifiers: 0,
                code: 24,
            },
        ),
        (
            'v',
            PasteKey {
                modifiers: 0,
                code: 25,
            },
        ),
        (
            'w',
            PasteKey {
                modifiers: 0,
                code: 26,
            },
        ),
        (
            'x',
            PasteKey {
                modifiers: 0,
                code: 27,
            },
        ),
        (
            'y',
            PasteKey {
                modifiers: 0,
                code: 28,
            },
        ),
        (
            'z',
            PasteKey {
                modifiers: 0,
                code: 29,
            },
        ),
        (
            'A',
            PasteKey {
                modifiers: 2,
                code: 4,
            },
        ),
        (
            'B',
            PasteKey {
                modifiers: 2,
                code: 5,
            },
        ),
        (
            'C',
            PasteKey {
                modifiers: 2,
                code: 6,
            },
        ),
        (
            'D',
            PasteKey {
                modifiers: 2,
                code: 7,
            },
        ),
        (
            'E',
            PasteKey {
                modifiers: 2,
                code: 8,
            },
        ),
        (
            'F',
            PasteKey {
                modifiers: 2,
                code: 9,
            },
        ),
        (
            'G',
            PasteKey {
                modifiers: 2,
                code: 10,
            },
        ),
        (
            'H',
            PasteKey {
                modifiers: 2,
                code: 11,
            },
        ),
        (
            'I',
            PasteKey {
                modifiers: 2,
                code: 12,
            },
        ),
        (
            'J',
            PasteKey {
                modifiers: 2,
                code: 13,
            },
        ),
        (
            'K',
            PasteKey {
                modifiers: 2,
                code: 14,
            },
        ),
        (
            'L',
            PasteKey {
                modifiers: 2,
                code: 15,
            },
        ),
        (
            'M',
            PasteKey {
                modifiers: 2,
                code: 16,
            },
        ),
        (
            'N',
            PasteKey {
                modifiers: 2,
                code: 17,
            },
        ),
        (
            'O',
            PasteKey {
                modifiers: 2,
                code: 18,
            },
        ),
        (
            'P',
            PasteKey {
                modifiers: 2,
                code: 19,
            },
        ),
        (
            'Q',
            PasteKey {
                modifiers: 2,
                code: 20,
            },
        ),
        (
            'R',
            PasteKey {
                modifiers: 2,
                code: 21,
            },
        ),
        (
            'S',
            PasteKey {
                modifiers: 2,
                code: 22,
            },
        ),
        (
            'T',
            PasteKey {
                modifiers: 2,
                code: 23,
            },
        ),
        (
            'U',
            PasteKey {
                modifiers: 2,
                code: 24,
            },
        ),
        (
            'V',
            PasteKey {
                modifiers: 2,
                code: 25,
            },
        ),
        (
            'W',
            PasteKey {
                modifiers: 2,
                code: 26,
            },
        ),
        (
            'X',
            PasteKey {
                modifiers: 2,
                code: 27,
            },
        ),
        (
            'Y',
            PasteKey {
                modifiers: 2,
                code: 28,
            },
        ),
        (
            'Z',
            PasteKey {
                modifiers: 2,
                code: 29,
            },
        ),
        (
            '1',
            PasteKey {
                modifiers: 0,
                code: 30,
            },
        ),
        (
            '2',
            PasteKey {
                modifiers: 0,
                code: 31,
            },
        ),
        (
            '3',
            PasteKey {
                modifiers: 0,
                code: 32,
            },
        ),
        (
            '4',
            PasteKey {
                modifiers: 0,
                code: 33,
            },
        ),
        (
            '5',
            PasteKey {
                modifiers: 0,
                code: 34,
            },
        ),
        (
            '6',
            PasteKey {
                modifiers: 0,
                code: 35,
            },
        ),
        (
            '7',
            PasteKey {
                modifiers: 0,
                code: 36,
            },
        ),
        (
            '8',
            PasteKey {
                modifiers: 0,
                code: 37,
            },
        ),
        (
            '9',
            PasteKey {
                modifiers: 0,
                code: 38,
            },
        ),
        (
            '0',
            PasteKey {
                modifiers: 0,
                code: 39,
            },
        ),
        (
            '!',
            PasteKey {
                modifiers: 2,
                code: 30,
            },
        ),
        (
            '@',
            PasteKey {
                modifiers: 2,
                code: 31,
            },
        ),
        (
            '#',
            PasteKey {
                modifiers: 2,
                code: 32,
            },
        ),
        (
            '$',
            PasteKey {
                modifiers: 2,
                code: 33,
            },
        ),
        (
            '%',
            PasteKey {
                modifiers: 2,
                code: 34,
            },
        ),
        (
            '^',
            PasteKey {
                modifiers: 2,
                code: 35,
            },
        ),
        (
            '&',
            PasteKey {
                modifiers: 2,
                code: 36,
            },
        ),
        (
            '*',
            PasteKey {
                modifiers: 2,
                code: 37,
            },
        ),
        (
            '(',
            PasteKey {
                modifiers: 2,
                code: 38,
            },
        ),
        (
            ')',
            PasteKey {
                modifiers: 2,
                code: 39,
            },
        ),
        (
            '\n',
            PasteKey {
                modifiers: 0,
                code: 40,
            },
        ),
        (
            '\t',
            PasteKey {
                modifiers: 0,
                code: 43,
            },
        ),
        (
            ' ',
            PasteKey {
                modifiers: 0,
                code: 44,
            },
        ),
        (
            '-',
            PasteKey {
                modifiers: 0,
                code: 45,
            },
        ),
        (
            '=',
            PasteKey {
                modifiers: 0,
                code: 46,
            },
        ),
        (
            '[',
            PasteKey {
                modifiers: 0,
                code: 47,
            },
        ),
        (
            ']',
            PasteKey {
                modifiers: 0,
                code: 48,
            },
        ),
        (
            '\\',
            PasteKey {
                modifiers: 0,
                code: 49,
            },
        ),
        (
            ';',
            PasteKey {
                modifiers: 0,
                code: 51,
            },
        ),
        (
            '\'',
            PasteKey {
                modifiers: 0,
                code: 52,
            },
        ),
        (
            '`',
            PasteKey {
                modifiers: 0,
                code: 53,
            },
        ),
        (
            ',',
            PasteKey {
                modifiers: 0,
                code: 54,
            },
        ),
        (
            '.',
            PasteKey {
                modifiers: 0,
                code: 55,
            },
        ),
        (
            '/',
            PasteKey {
                modifiers: 0,
                code: 56,
            },
        ),
        (
            '_',
            PasteKey {
                modifiers: 2,
                code: 45,
            },
        ),
        (
            '+',
            PasteKey {
                modifiers: 2,
                code: 46,
            },
        ),
        (
            '{',
            PasteKey {
                modifiers: 2,
                code: 47,
            },
        ),
        (
            '}',
            PasteKey {
                modifiers: 2,
                code: 48,
            },
        ),
        (
            '|',
            PasteKey {
                modifiers: 2,
                code: 49,
            },
        ),
        (
            ':',
            PasteKey {
                modifiers: 2,
                code: 51,
            },
        ),
        (
            '"',
            PasteKey {
                modifiers: 2,
                code: 52,
            },
        ),
        (
            '~',
            PasteKey {
                modifiers: 2,
                code: 53,
            },
        ),
        (
            '<',
            PasteKey {
                modifiers: 2,
                code: 54,
            },
        ),
        (
            '>',
            PasteKey {
                modifiers: 2,
                code: 55,
            },
        ),
        (
            '?',
            PasteKey {
                modifiers: 2,
                code: 56,
            },
        ),
    ]);

    if layout == KeyboardLayout::De {
        map.insert(
            'y',
            PasteKey {
                modifiers: 0,
                code: 29,
            },
        );
        map.insert(
            'Y',
            PasteKey {
                modifiers: 2,
                code: 29,
            },
        );
        map.insert(
            'z',
            PasteKey {
                modifiers: 0,
                code: 28,
            },
        );
        map.insert(
            'Z',
            PasteKey {
                modifiers: 2,
                code: 28,
            },
        );
        map.insert(
            '\u{00E4}',
            PasteKey {
                modifiers: 0,
                code: 52,
            },
        );
        map.insert(
            '\u{00C4}',
            PasteKey {
                modifiers: 2,
                code: 52,
            },
        );
        map.insert(
            '\u{00F6}',
            PasteKey {
                modifiers: 0,
                code: 51,
            },
        );
        map.insert(
            '\u{00D6}',
            PasteKey {
                modifiers: 2,
                code: 51,
            },
        );
        map.insert(
            '\u{00FC}',
            PasteKey {
                modifiers: 0,
                code: 47,
            },
        );
        map.insert(
            '\u{00DC}',
            PasteKey {
                modifiers: 2,
                code: 47,
            },
        );
        map.insert(
            '\u{00DF}',
            PasteKey {
                modifiers: 0,
                code: 45,
            },
        );
        map.insert(
            '^',
            PasteKey {
                modifiers: 0,
                code: 53,
            },
        );
        map.insert(
            '/',
            PasteKey {
                modifiers: 2,
                code: 36,
            },
        );
        map.insert(
            '(',
            PasteKey {
                modifiers: 2,
                code: 37,
            },
        );
        map.insert(
            '&',
            PasteKey {
                modifiers: 2,
                code: 35,
            },
        );
        map.insert(
            ')',
            PasteKey {
                modifiers: 2,
                code: 38,
            },
        );
        map.insert(
            '`',
            PasteKey {
                modifiers: 2,
                code: 46,
            },
        );
        map.insert(
            '"',
            PasteKey {
                modifiers: 2,
                code: 31,
            },
        );
        map.insert(
            '?',
            PasteKey {
                modifiers: 2,
                code: 45,
            },
        );
        map.insert(
            '{',
            PasteKey {
                modifiers: 0x40,
                code: 36,
            },
        );
        map.insert(
            '[',
            PasteKey {
                modifiers: 0x40,
                code: 37,
            },
        );
        map.insert(
            ']',
            PasteKey {
                modifiers: 0x40,
                code: 38,
            },
        );
        map.insert(
            '}',
            PasteKey {
                modifiers: 0x40,
                code: 39,
            },
        );
        map.insert(
            '\\',
            PasteKey {
                modifiers: 0x40,
                code: 45,
            },
        );
        map.insert(
            '@',
            PasteKey {
                modifiers: 0x40,
                code: 20,
            },
        );
        map.insert(
            '+',
            PasteKey {
                modifiers: 0,
                code: 48,
            },
        );
        map.insert(
            '*',
            PasteKey {
                modifiers: 2,
                code: 48,
            },
        );
        map.insert(
            '~',
            PasteKey {
                modifiers: 0x40,
                code: 48,
            },
        );
        map.insert(
            '#',
            PasteKey {
                modifiers: 0,
                code: 49,
            },
        );
        map.insert(
            '\'',
            PasteKey {
                modifiers: 2,
                code: 49,
            },
        );
        map.insert(
            '<',
            PasteKey {
                modifiers: 0,
                code: 100,
            },
        );
        map.insert(
            '>',
            PasteKey {
                modifiers: 2,
                code: 100,
            },
        );
        map.insert(
            '|',
            PasteKey {
                modifiers: 0x40,
                code: 100,
            },
        );
        map.insert(
            ';',
            PasteKey {
                modifiers: 2,
                code: 54,
            },
        );
        map.insert(
            ':',
            PasteKey {
                modifiers: 2,
                code: 55,
            },
        );
        map.insert(
            '-',
            PasteKey {
                modifiers: 0,
                code: 56,
            },
        );
        map.insert(
            '_',
            PasteKey {
                modifiers: 2,
                code: 56,
            },
        );
        map.insert(
            '\u{00B4}',
            PasteKey {
                modifiers: 0,
                code: 46,
            },
        );
        map.insert(
            '\u{00B0}',
            PasteKey {
                modifiers: 2,
                code: 53,
            },
        );
        map.insert(
            '\u{00A7}',
            PasteKey {
                modifiers: 2,
                code: 32,
            },
        );
        map.insert(
            '\u{20AC}',
            PasteKey {
                modifiers: 0x40,
                code: 8,
            },
        );
        map.insert(
            '\u{00B2}',
            PasteKey {
                modifiers: 0x40,
                code: 31,
            },
        );
        map.insert(
            '\u{00B3}',
            PasteKey {
                modifiers: 0x40,
                code: 32,
            },
        );
    }

    map
}

fn paste_layout_or_default(layout: Option<KeyboardLayout>) -> KeyboardLayout {
    layout.unwrap_or(KeyboardLayout::Us)
}

fn send_keyboard_stroke(modifiers: u8, code: u8) -> Result<(), String> {
    let key_down = [modifiers, 0x00, code, 0x00, 0x00, 0x00, 0x00, 0x00];
    let key_up = [0u8; 8];
    write_hid_report(HID_DEVICE_KEYBOARD, &key_down)?;
    write_hid_report(HID_DEVICE_KEYBOARD, &key_up)?;
    thread::sleep(Duration::from_millis(30));
    Ok(())
}

fn reset_hid_via_script() -> Result<(), String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("{HID_MODE_TARGET_SCRIPT} restart_phy"))
        .output()
        .map_err(|err| format!("failed to execute hid reset script: {err}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!(
            "hid reset command failed(status={}): stdout='{}' stderr='{}'",
            output.status,
            stdout.trim(),
            stderr.trim()
        ))
    }
}

fn copy_script_file(source: &str, target: &str) -> Result<(), String> {
    let copied =
        std::fs::copy(source, target).map_err(|err| format!("failed to copy {source}: {err}"))?;
    if copied == 0 {
        return Err(format!("failed to copy {source}: copied zero bytes"));
    }
    Ok(())
}

impl crate::imago_nanokvm_plugin_bindings::imago::nanokvm::hid_control::Host for ResourceTable {
    fn get_hid_mode(&mut self) -> Result<HidMode, String> {
        ensure_nanokvm_environment()?;
        let mode_flag = read_file_trimmed(USB_MODE_FLAG_PATH)?;
        parse_hid_mode_from_usb_mode(&mode_flag)
    }

    fn set_hid_mode(&mut self, mode: HidMode) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        let source = hid_mode_script_source(mode)?;
        copy_script_file(source, HID_MODE_TARGET_SCRIPT)
    }

    fn send_keyboard(&mut self, event: KeyboardEvent) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        let report = build_keyboard_report(&event)?;
        write_hid_report(HID_DEVICE_KEYBOARD, &report)
    }

    fn send_mouse_relative(&mut self, event: RelativeMouseEvent) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        let report = build_relative_mouse_report(&event);
        write_hid_report(HID_DEVICE_RELATIVE_MOUSE, &report)
    }

    fn send_mouse_absolute(&mut self, event: AbsoluteMouseEvent) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        let report = build_absolute_mouse_report(&event);
        write_hid_report(HID_DEVICE_ABSOLUTE_MOUSE, &report)
    }

    fn send_touch(&mut self, event: TouchEvent) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        let report = build_touch_report(&event);
        write_hid_report(HID_DEVICE_ABSOLUTE_MOUSE, &report)
    }

    fn paste(&mut self, content: String, layout: Option<KeyboardLayout>) -> Result<u32, String> {
        ensure_nanokvm_environment()?;
        if content.chars().count() > 1024 {
            return Err("paste content is too long (max 1024 chars)".to_string());
        }

        let map = paste_char_map(paste_layout_or_default(layout));
        let mut written: u32 = 0;
        for ch in content.chars() {
            let Some(key) = map.get(&ch) else {
                continue;
            };
            send_keyboard_stroke(key.modifiers, key.code)?;
            written = written
                .checked_add(1)
                .ok_or_else(|| "paste character count overflow".to_string())?;
        }
        Ok(written)
    }

    fn reset_hid(&mut self) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        reset_hid_via_script()
    }
}
