use imagod_runtime_wasmtime::native_plugins::NativePlugin;

use crate::{
    ImagoNanoKvmPlugin,
    capture::read_first_mjpeg_frame,
    device_status::{parse_link_status, parse_usb_mode},
    hid_control::{
        build_absolute_mouse_report, build_keyboard_report, build_relative_mouse_report,
        build_touch_report, hid_mode_script_source,
    },
    io_control::{gpio_path_for, pulse_duration_ms},
    runtime_control::{
        parse_stop_ping_toggle, parse_watchdog_toggle, stop_ping_script_source,
        watchdog_script_source,
    },
    session::{
        NanoKvmSession, SessionAuth, lookup_session, normalize_endpoint, parse_login_response,
        register_session, remove_session, resolve_auth_cookie,
    },
    stream_config::validate_fps,
    stream_config::validate_quality,
    types::{
        AbsoluteMouseEvent, GpioPulseKind, HardwareKind, HdmiStatus, HidMode, KeyboardEvent,
        LinkStatus, RelativeMouseEvent, ToggleState, TouchEvent, UsbMode,
    },
};

#[test]
fn supports_all_imports_and_symbol_prefixes() {
    let plugin = ImagoNanoKvmPlugin;
    assert_eq!(ImagoNanoKvmPlugin::PACKAGE_NAME, "imago:nanokvm");
    assert_eq!(ImagoNanoKvmPlugin::IMPORTS.len(), 6);
    assert!(plugin.supports_import("imago:nanokvm/capture@0.1.0"));
    assert!(plugin.supports_import("imago:nanokvm/stream-config@0.1.0"));
    assert!(plugin.supports_import("imago:nanokvm/device-status@0.1.0"));
    assert!(plugin.supports_import("imago:nanokvm/runtime-control@0.1.0"));
    assert!(plugin.supports_import("imago:nanokvm/hid-control@0.1.0"));
    assert!(plugin.supports_import("imago:nanokvm/io-control@0.1.0"));

    assert!(plugin.supports_symbol("imago:nanokvm/stream-config@0.1.0.get-settings"));
    assert!(plugin.supports_symbol("imago:nanokvm/device-status@0.1.0.get-usb-mode"));
    assert!(plugin.supports_symbol("imago:nanokvm/runtime-control@0.1.0.get-watchdog"));
    assert!(plugin.supports_symbol("imago:nanokvm/hid-control@0.1.0.send-keyboard"));
    assert!(plugin.supports_symbol("imago:nanokvm/io-control@0.1.0.power-pulse"));
}

#[test]
fn normalize_endpoint_accepts_http_host_and_default_port() {
    let normalized = normalize_endpoint("http://Example.com").expect("endpoint should parse");
    assert_eq!(normalized, "http://example.com:80");
}

#[test]
fn normalize_endpoint_rejects_non_http_scheme() {
    let err = normalize_endpoint("https://example.com").expect_err("https must fail");
    assert!(err.contains("http://"), "unexpected error: {err}");
}

#[test]
fn normalize_endpoint_rejects_missing_host() {
    let err = normalize_endpoint("http://:80").expect_err("missing host must fail");
    assert!(
        err.contains("host") || err.contains("invalid endpoint url"),
        "unexpected error: {err}"
    );
}

#[test]
fn resolve_auth_cookie_handles_none_token_and_login() {
    let none_cookie =
        resolve_auth_cookie("http://127.0.0.1:80", SessionAuth::None, |_e, _u, _p| {
            panic!("login provider should not be called for none")
        })
        .expect("none auth should succeed");
    assert_eq!(none_cookie, None);

    let token_cookie = resolve_auth_cookie(
        "http://127.0.0.1:80",
        SessionAuth::Token("abc".to_string()),
        |_e, _u, _p| panic!("login provider should not be called for token"),
    )
    .expect("token auth should succeed");
    assert_eq!(token_cookie, Some("nano-kvm-token=abc".to_string()));

    let login_cookie = resolve_auth_cookie(
        "http://127.0.0.1:80",
        SessionAuth::Login {
            username: "admin".to_string(),
            password: "secret".to_string(),
        },
        |endpoint, username, password| {
            assert_eq!(endpoint, "http://127.0.0.1:80");
            assert_eq!(username, "admin");
            assert_eq!(password, "secret");
            Ok("login-token".to_string())
        },
    )
    .expect("login auth should succeed");
    assert_eq!(login_cookie, Some("nano-kvm-token=login-token".to_string()));
}

#[test]
fn parse_login_response_rejects_non_zero_code() {
    let err = parse_login_response(r#"{"code":-2,"msg":"invalid username or password"}"#)
        .expect_err("non-zero code should fail");
    assert!(err.contains("code=-2"), "unexpected error: {err}");
}

#[test]
fn parse_login_response_rejects_missing_token() {
    let err = parse_login_response(r#"{"code":0,"msg":"success","data":{}}"#)
        .expect_err("missing token should fail");
    assert!(err.contains("data.token"), "unexpected error: {err}");
}

#[test]
fn parse_login_response_accepts_success_token() {
    let token = parse_login_response(r#"{"code":0,"msg":"success","data":{"token":"abc"}}"#)
        .expect("token should parse");
    assert_eq!(token, "abc");
}

#[test]
fn read_first_mjpeg_frame_extracts_jpeg_payload() {
    let sample = concat!(
        "--frame\r\n",
        "Content-Type: image/jpeg\r\n",
        "Content-Length: 4\r\n",
        "\r\n"
    )
    .as_bytes()
    .iter()
    .copied()
    .chain([0xff, 0xd8, 0xff, 0xd9])
    .collect::<Vec<u8>>();

    let frame =
        read_first_mjpeg_frame(std::io::Cursor::new(sample), "frame").expect("frame should parse");
    assert_eq!(frame, vec![0xff, 0xd8, 0xff, 0xd9]);
}

#[test]
fn read_first_mjpeg_frame_rejects_overlong_line_without_newline() {
    let sample = vec![b'a'; crate::constants::MAX_MJPEG_HEADER_LINE_BYTES + 1];
    let err = read_first_mjpeg_frame(std::io::Cursor::new(sample), "frame")
        .expect_err("overlong boundary line should fail");
    assert!(
        err.contains("mjpeg boundary line exceeds maximum size"),
        "unexpected error: {err}"
    );
}

#[test]
fn read_first_mjpeg_frame_rejects_missing_header_terminator_within_max_lines() {
    let mut sample = String::from("--frame\r\nContent-Length: 4\r\n");
    for _ in 1..crate::constants::MAX_MJPEG_HEADER_LINES {
        sample.push_str("X-Dummy: value\r\n");
    }

    let err = read_first_mjpeg_frame(std::io::Cursor::new(sample.into_bytes()), "frame")
        .expect_err("missing header terminator should fail");
    assert!(
        err.contains("header terminator not found"),
        "unexpected error: {err}"
    );
}

#[test]
fn parse_status_enums_reject_unknown_values() {
    assert_eq!(
        parse_usb_mode("0x0510").expect("valid usb"),
        UsbMode::Normal
    );
    assert_eq!(
        crate::device_status::parse_hdmi_status("1").expect("valid hdmi"),
        HdmiStatus::Normal
    );
    assert_eq!(
        parse_link_status("up", "ethernet").expect("valid link"),
        LinkStatus::Connected
    );

    assert!(
        parse_usb_mode("0x9999")
            .expect_err("unknown usb should fail")
            .contains("unknown usb mode")
    );
    assert!(
        crate::device_status::parse_hdmi_status("9")
            .expect_err("unknown hdmi should fail")
            .contains("unknown hdmi status")
    );
    assert!(
        parse_link_status("dormant", "wifi")
            .expect_err("unknown link should fail")
            .contains("unknown wifi link")
    );
}

#[test]
fn validate_stream_limits() {
    validate_fps(10).expect("fps lower bound should pass");
    validate_fps(60).expect("fps upper bound should pass");
    validate_quality(50).expect("quality lower bound should pass");
    validate_quality(100).expect("quality upper bound should pass");

    assert!(validate_fps(9).is_err());
    assert!(validate_fps(61).is_err());
    assert!(validate_quality(49).is_err());
    assert!(validate_quality(101).is_err());
}

#[test]
fn runtime_toggle_parsers_and_script_sources() {
    let enabled_watchdog = "#!/bin/sh\nwhile true ; do\n";
    let disabled_watchdog = "#!/bin/sh\n#while true ; do\n";
    assert_eq!(
        parse_watchdog_toggle(enabled_watchdog).expect("watchdog enabled"),
        ToggleState::Enabled
    );
    assert_eq!(
        parse_watchdog_toggle(disabled_watchdog).expect("watchdog disabled"),
        ToggleState::Disabled
    );

    let enabled_stop_ping = "#!/bin/sh\n(sleep 5;touch /tmp/stop; rm -rf /tmp/stop)&\n";
    let disabled_stop_ping = "#!/bin/sh\n#(sleep 5;touch /tmp/stop; rm -rf /tmp/stop)&\n";
    assert_eq!(
        parse_stop_ping_toggle(enabled_stop_ping).expect("stop ping enabled"),
        ToggleState::Enabled
    );
    assert_eq!(
        parse_stop_ping_toggle(disabled_stop_ping).expect("stop ping disabled"),
        ToggleState::Disabled
    );

    assert_eq!(
        watchdog_script_source(ToggleState::Enabled),
        crate::constants::RUNTIME_SCRIPT_WATCHDOG_ENABLED
    );
    assert_eq!(
        watchdog_script_source(ToggleState::Disabled),
        crate::constants::RUNTIME_SCRIPT_WATCHDOG_DISABLED
    );
    assert_eq!(
        stop_ping_script_source(ToggleState::Enabled),
        crate::constants::RUNTIME_SCRIPT_STOP_PING_ENABLED
    );
    assert_eq!(
        stop_ping_script_source(ToggleState::Disabled),
        crate::constants::RUNTIME_SCRIPT_STOP_PING_DISABLED
    );
}

#[test]
fn hid_reports_are_built_with_expected_shapes() {
    let keyboard = build_keyboard_report(&KeyboardEvent {
        modifiers: 0x02,
        keycodes: vec![0x04, 0x05, 0x06],
    })
    .expect("keyboard report should build");
    assert_eq!(keyboard, [0x02, 0x00, 0x04, 0x05, 0x06, 0x00, 0x00, 0x00]);

    let relative = build_relative_mouse_report(&RelativeMouseEvent {
        buttons: 0x01,
        dx: 200,
        dy: -200,
        wheel: 3,
    });
    assert_eq!(relative, [0x01, 127u8, 129u8, 3u8]);

    let absolute = build_absolute_mouse_report(&AbsoluteMouseEvent {
        buttons: 0x03,
        x: 0x1234,
        y: 0xabcd,
        wheel: -200,
    });
    assert_eq!(absolute, [0x03, 0x34, 0x12, 0xcd, 0xab, 129u8]);

    let touch = build_touch_report(&TouchEvent {
        pressed: true,
        x: 100,
        y: 200,
    });
    assert_eq!(touch, [1, 100, 0, 200, 0, 0]);
}

#[test]
fn hid_mode_supports_only_known_switch_paths() {
    assert_eq!(
        hid_mode_script_source(HidMode::Hid).expect("hid should map"),
        crate::constants::HID_MODE_HID_ONLY_SCRIPT
    );
    assert_eq!(
        hid_mode_script_source(HidMode::HidAndMouse).expect("hid-and-mouse should map"),
        crate::constants::HID_MODE_NORMAL_SCRIPT
    );
    assert!(hid_mode_script_source(HidMode::HidAndTouchpad).is_err());
    assert!(hid_mode_script_source(HidMode::HidAndAbsoluteMouse).is_err());
}

#[test]
fn gpio_resolution_and_pulse_duration_follow_hw_rules() {
    assert_eq!(
        gpio_path_for(HardwareKind::Alpha, GpioPulseKind::Power),
        crate::constants::GPIO_POWER_ALPHA_BETA_PCIE
    );
    assert_eq!(
        gpio_path_for(HardwareKind::Alpha, GpioPulseKind::Reset),
        crate::constants::GPIO_RESET_ALPHA
    );
    assert_eq!(
        gpio_path_for(HardwareKind::Beta, GpioPulseKind::Reset),
        crate::constants::GPIO_RESET_BETA_PCIE
    );
    assert_eq!(
        gpio_path_for(HardwareKind::Pcie, GpioPulseKind::Reset),
        crate::constants::GPIO_RESET_BETA_PCIE
    );

    assert_eq!(pulse_duration_ms(None), 800);
    assert_eq!(pulse_duration_ms(Some(0)), 800);
    assert_eq!(pulse_duration_ms(Some(1200)), 1200);
}

#[test]
fn session_registry_add_lookup_remove() {
    let rep = register_session(NanoKvmSession {
        endpoint: "http://127.0.0.1:80".to_string(),
        cookie_header: None,
    });

    let session = lookup_session(rep).expect("session should exist");
    assert_eq!(session.endpoint, "http://127.0.0.1:80");

    remove_session(rep);
    let err = lookup_session(rep).expect_err("session should be removed");
    assert!(err.contains("not found"), "unexpected error: {err}");
}
