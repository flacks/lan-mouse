use async_trait::async_trait;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, InputId, KeyCode, PropType,
    RelativeAxisCode, UinputAbsSetup, uinput::VirtualDevice,
};
use input_event::{GestureEvent, KeyboardEvent, PointerEvent};
use std::collections::HashMap;

use crate::{Emulation, EmulationError, EmulationHandle, error::EvdevEmulationCreationError};

const WHEEL_SENSITIVITY: f64 = 3.0;

// Get pointer motion scale from environment or use default
fn get_pointer_motion_scale() -> f64 {
    std::env::var("LAN_MOUSE_POINTER_SCALE")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.5)
}

// Virtual touchpad dimensions (logical units)
const TOUCHPAD_WIDTH: i32 = 1000;
const TOUCHPAD_HEIGHT: i32 = 600;

// Gesture state tracking
struct GestureState {
    active: bool,
    fingers: u8,
    // Current position for each finger (x, y)
    positions: [(f64, f64); 5],
}

impl GestureState {
    fn new() -> Self {
        Self {
            active: false,
            fingers: 0,
            positions: [(TOUCHPAD_WIDTH as f64 / 2.0, TOUCHPAD_HEIGHT as f64 / 2.0); 5],
        }
    }
}

pub(crate) struct EvdevEmulation {
    mouse_dev: VirtualDevice,
    touchpad_dev: VirtualDevice,
    gesture_state: Box<GestureState>,
    pointer_motion_scale: f64,
    per_handle_scales: HashMap<EmulationHandle, f64>,
}

impl EvdevEmulation {
    pub fn new() -> Result<Self, EvdevEmulationCreationError> {
        log::info!("Creating evdev emulation devices...");
        
        // Create pointer device that acts like a touchpad for libinput acceleration
        // but uses relative motion like a mouse (not absolute position)
        let mouse_dev = VirtualDevice::builder()?
            .name("lan-mouse")
            // identify as a USB touchpad so libinput applies pointer acceleration
            .input_id(InputId::new(BusType(0x03), 0x1234, 0x5678, 0x0001))
            .with_properties(&AttributeSet::from_iter([PropType::POINTER]))?
            // All keys including touchpad buttons so libinput detects it as touchpad
            .with_keys(&AttributeSet::from_iter(ALL_KEYS.iter().chain(&[
                KeyCode::BTN_TOUCH,
                KeyCode::BTN_TOOL_FINGER,
            ]).copied()))?
            .with_relative_axes(&AttributeSet::from_iter([
                RelativeAxisCode::REL_X,
                RelativeAxisCode::REL_Y,
                RelativeAxisCode::REL_WHEEL,
                RelativeAxisCode::REL_HWHEEL,
                RelativeAxisCode::REL_WHEEL_HI_RES,
                RelativeAxisCode::REL_HWHEEL_HI_RES,
            ]))?
            .build()?;
        log::info!("✓ Created lan-mouse device (pointer with touchpad acceleration)");

        // Create separate touchpad device only for multitouch gestures
        let touchpad_dev = VirtualDevice::builder()?
            .name("lan-mouse-gestures")
            // identify as a USB touchpad
            .input_id(InputId::new(BusType(0x03), 0x1234, 0x5679, 0x0001))
            .with_properties(&AttributeSet::from_iter([PropType::POINTER]))?
            // Only touchpad gesture buttons, no regular mouse buttons
            .with_keys(&AttributeSet::from_iter([
                KeyCode::BTN_TOUCH,
                KeyCode::BTN_TOOL_FINGER,
                KeyCode::BTN_TOOL_DOUBLETAP,
                KeyCode::BTN_TOOL_TRIPLETAP,
                KeyCode::BTN_TOOL_QUADTAP,
                KeyCode::BTN_TOOL_QUINTTAP,
            ]))?
            // Multitouch axes (Protocol B) - no relative axes
            .with_absolute_axis(&UinputAbsSetup::new(
                AbsoluteAxisCode::ABS_MT_SLOT,
                AbsInfo::new(0, 0, 4, 0, 0, 1),
            ))?
            .with_absolute_axis(&UinputAbsSetup::new(
                AbsoluteAxisCode::ABS_MT_TRACKING_ID,
                AbsInfo::new(0, -1, 65535, 0, 0, 1),
            ))?
            .with_absolute_axis(&UinputAbsSetup::new(
                AbsoluteAxisCode::ABS_MT_POSITION_X,
                AbsInfo::new(0, 0, TOUCHPAD_WIDTH, 0, 0, 10),
            ))?
            .with_absolute_axis(&UinputAbsSetup::new(
                AbsoluteAxisCode::ABS_MT_POSITION_Y,
                AbsInfo::new(0, 0, TOUCHPAD_HEIGHT, 0, 0, 10),
            ))?
            .build()?;
        log::info!("✓ Created lan-mouse-gestures device (multitouch only)");

        let pointer_motion_scale = get_pointer_motion_scale();
        log::info!("Evdev emulation ready: pointer + gestures (default motion scale: {:.2})", pointer_motion_scale);
        Ok(EvdevEmulation {
            mouse_dev,
            touchpad_dev,
            gesture_state: Box::new(GestureState::new()),
            pointer_motion_scale,
            per_handle_scales: HashMap::new(),
        })
    }
}

#[async_trait]
impl Emulation for EvdevEmulation {
    async fn consume(
        &mut self,
        event: input_event::Event,
        handle: EmulationHandle,
    ) -> Result<(), EmulationError> {
        match event {
            input_event::Event::Pointer(p) => {
                log::trace!("[evdev] Pointer event: {:?}", p);
                match p {
                PointerEvent::Motion { time: _, dx, dy } => {
                    // Get per-handle scale or use default
                    let scale = self.per_handle_scales.get(&handle).copied().unwrap_or(self.pointer_motion_scale);
                    // Scale motion down so libinput acceleration works in a reasonable range
                    let scaled_dx = (dx * scale).round() as i32;
                    let scaled_dy = (dy * scale).round() as i32;
                    log::trace!("[evdev] Motion: handle={}, scale={:.2}, dx={}->{}, dy={}->{}", 
                               handle, scale, dx, scaled_dx, dy, scaled_dy);
                    self.mouse_dev.emit(&[
                        *evdev::RelativeAxisEvent::new(RelativeAxisCode::REL_X, scaled_dx),
                        *evdev::RelativeAxisEvent::new(RelativeAxisCode::REL_Y, scaled_dy),
                    ])?;
                }
                PointerEvent::Button {
                    time: _,
                    button,
                    state,
                } => {
                    self.mouse_dev
                        .emit(&[*evdev::KeyEvent::new(KeyCode(button as u16), state as i32)])?;
                }
                PointerEvent::Axis {
                    time: _,
                    axis,
                    value,
                } => {
                    let (axis_hi_res, axis_legacy) = match axis {
                        0 => (
                            RelativeAxisCode::REL_WHEEL_HI_RES,
                            RelativeAxisCode::REL_WHEEL,
                        ),
                        _ => (
                            RelativeAxisCode::REL_HWHEEL_HI_RES,
                            RelativeAxisCode::REL_HWHEEL,
                        ),
                    };

                    let hi_res = (value * WHEEL_SENSITIVITY).round() as i32;
                    // map hi-res ticks to legacy wheel steps (~120 units per detent)
                    let legacy = if hi_res >= 0 {
                        (hi_res + 60) / 120
                    } else {
                        (hi_res - 60) / 120
                    };

                    self.mouse_dev.emit(&[
                        *evdev::RelativeAxisEvent::new(axis_hi_res, hi_res),
                        *evdev::RelativeAxisEvent::new(axis_legacy, legacy),
                    ])?;
                }
                PointerEvent::AxisDiscrete120 { axis, value } => {
                    let (axis_hi_res, axis_legacy) = match axis {
                        0 => (
                            RelativeAxisCode::REL_WHEEL_HI_RES,
                            RelativeAxisCode::REL_WHEEL,
                        ),
                        _ => (
                            RelativeAxisCode::REL_HWHEEL_HI_RES,
                            RelativeAxisCode::REL_HWHEEL,
                        ),
                    };

                    let hi_res = value * 120;
                    let legacy = value;

                    self.mouse_dev.emit(&[
                        *evdev::RelativeAxisEvent::new(axis_hi_res, hi_res),
                        *evdev::RelativeAxisEvent::new(axis_legacy, legacy),
                    ])?;
                }
                }
            },
            input_event::Event::Keyboard(k) => {
                log::trace!("[evdev] Keyboard event: {:?}", k);
                match k {
                KeyboardEvent::Key {
                    time: _,
                    key,
                    state,
                } => {
                    self.mouse_dev
                        .emit(&[*evdev::KeyEvent::new(KeyCode(key as u16), state as i32)])?;
                }
                KeyboardEvent::Modifiers { .. } => {}
                }
            },
            input_event::Event::Gesture(g) => {
                self.handle_gesture(g)?;
            }
        }
        Ok(())
    }

    async fn create(&mut self, _: EmulationHandle) {}
    async fn destroy(&mut self, _: EmulationHandle) {}
    async fn terminate(&mut self) {}

    fn set_pointer_motion_scale(&mut self, handle: EmulationHandle, scale: Option<f64>) {
        if let Some(scale) = scale {
            log::debug!("Setting pointer motion scale for handle {}: {:.2}", handle, scale);
            self.per_handle_scales.insert(handle, scale);
        } else {
            log::debug!("Clearing pointer motion scale for handle {}, using default", handle);
            self.per_handle_scales.remove(&handle);
        }
    }
}

impl EvdevEmulation {
    fn handle_gesture(&mut self, gesture: GestureEvent) -> Result<(), EmulationError> {
        match gesture {
            GestureEvent::SwipeBegin { time: _, fingers } => {
                self.swipe_begin(fingers)?;
            }
            GestureEvent::SwipeUpdate { time: _, dx, dy } => {
                self.swipe_update(dx, dy)?;
            }
            GestureEvent::SwipeEnd {
                time: _,
                cancelled,
            } => {
                self.swipe_end(cancelled)?;
            }
        }
        Ok(())
    }

    fn swipe_begin(&mut self, fingers: u8) -> Result<(), EmulationError> {
        self.gesture_state.active = true;
        self.gesture_state.fingers = fingers.min(5);

        // Initialize finger positions in the center of the touchpad
        for i in 0..self.gesture_state.fingers as usize {
            self.gesture_state.positions[i] = (
                TOUCHPAD_WIDTH as f64 / 2.0,
                TOUCHPAD_HEIGHT as f64 / 2.0,
            );
        }
        log::debug!("  Initialized {} finger positions at center ({}, {})", 
                   self.gesture_state.fingers, TOUCHPAD_WIDTH / 2, TOUCHPAD_HEIGHT / 2);

        // Emit BTN_TOUCH and BTN_TOOL_* to indicate gesture start
        self.touchpad_dev
            .emit(&[*evdev::KeyEvent::new(KeyCode::BTN_TOUCH, 1)])?;
        log::debug!("  Emitted BTN_TOUCH = 1");

        // Set the appropriate BTN_TOOL_* based on finger count
        let tool_btn = match fingers {
            1 => KeyCode::BTN_TOOL_FINGER,
            2 => KeyCode::BTN_TOOL_DOUBLETAP,
            3 => KeyCode::BTN_TOOL_TRIPLETAP,
            4 => KeyCode::BTN_TOOL_QUADTAP,
            _ => KeyCode::BTN_TOOL_QUINTTAP,
        };
        self.touchpad_dev
            .emit(&[*evdev::KeyEvent::new(tool_btn, 1)])?;
        log::debug!("  Emitted {:?} = 1", tool_btn);

        // Emit multitouch events for each finger
        for slot in 0..self.gesture_state.fingers {
            let (x, y) = self.gesture_state.positions[slot as usize];

            self.touchpad_dev.emit(&[
                *evdev::AbsoluteAxisEvent::new(
                    AbsoluteAxisCode::ABS_MT_SLOT,
                    slot as i32,
                ),
                *evdev::AbsoluteAxisEvent::new(
                    AbsoluteAxisCode::ABS_MT_TRACKING_ID,
                    slot as i32,
                ),
                *evdev::AbsoluteAxisEvent::new(
                    AbsoluteAxisCode::ABS_MT_POSITION_X,
                    x.round() as i32,
                ),
                *evdev::AbsoluteAxisEvent::new(
                    AbsoluteAxisCode::ABS_MT_POSITION_Y,
                    y.round() as i32,
                ),
            ])?;
            log::debug!("  Slot {}: tracking_id={}, pos=({}, {})", slot, slot, x.round() as i32, y.round() as i32);
        }

        Ok(())
    }

    fn swipe_update(&mut self, dx: f64, dy: f64) -> Result<(), EmulationError> {
        if !self.gesture_state.active {
            return Ok(());
        }

        // Update all finger positions
        for i in 0..self.gesture_state.fingers as usize {
            let (x, y) = &mut self.gesture_state.positions[i];
            let old_x = *x;
            let old_y = *y;
            *x += dx;
            *y += dy;

            // Clamp to touchpad bounds
            *x = x.clamp(0.0, TOUCHPAD_WIDTH as f64);
            *y = y.clamp(0.0, TOUCHPAD_HEIGHT as f64);
            
            log::trace!("  Finger {}: ({:.1}, {:.1}) -> ({:.1}, {:.1})", i, old_x, old_y, *x, *y);
        }

        // Emit updated positions for all fingers
        for slot in 0..self.gesture_state.fingers {
            let (x, y) = self.gesture_state.positions[slot as usize];

            self.touchpad_dev.emit(&[
                *evdev::AbsoluteAxisEvent::new(
                    AbsoluteAxisCode::ABS_MT_SLOT,
                    slot as i32,
                ),
                *evdev::AbsoluteAxisEvent::new(
                    AbsoluteAxisCode::ABS_MT_POSITION_X,
                    x.round() as i32,
                ),
                *evdev::AbsoluteAxisEvent::new(
                    AbsoluteAxisCode::ABS_MT_POSITION_Y,
                    y.round() as i32,
                ),
            ])?;
        }

        Ok(())
    }

    fn swipe_end(&mut self, cancelled: bool) -> Result<(), EmulationError> {
        // Release all tracking IDs
        for slot in 0..self.gesture_state.fingers {
            self.touchpad_dev.emit(&[
                *evdev::AbsoluteAxisEvent::new(
                    AbsoluteAxisCode::ABS_MT_SLOT,
                    slot as i32,
                ),
                *evdev::AbsoluteAxisEvent::new(
                    AbsoluteAxisCode::ABS_MT_TRACKING_ID,
                    -1,
                ),
            ])?;
            log::debug!("  Released tracking ID for slot {}", slot);
        }

        // Release BTN_TOOL_* based on current finger count
        let tool_btn = match self.gesture_state.fingers {
            1 => KeyCode::BTN_TOOL_FINGER,
            2 => KeyCode::BTN_TOOL_DOUBLETAP,
            3 => KeyCode::BTN_TOOL_TRIPLETAP,
            4 => KeyCode::BTN_TOOL_QUADTAP,
            _ => KeyCode::BTN_TOOL_QUINTTAP,
        };
        self.touchpad_dev
            .emit(&[*evdev::KeyEvent::new(tool_btn, 0)])?;
        log::debug!("  Released {:?}", tool_btn);

        // Release BTN_TOUCH
        self.touchpad_dev
            .emit(&[*evdev::KeyEvent::new(KeyCode::BTN_TOUCH, 0)])?;
        log::debug!("  Released BTN_TOUCH");

        self.gesture_state.active = false;
        self.gesture_state.fingers = 0;

        Ok(())
    }
}

const ALL_KEYS: [KeyCode; 557] = [
    // KeyCode::KEY_RESERVED,
    KeyCode::KEY_ESC,
    KeyCode::KEY_1,
    KeyCode::KEY_2,
    KeyCode::KEY_3,
    KeyCode::KEY_4,
    KeyCode::KEY_5,
    KeyCode::KEY_6,
    KeyCode::KEY_7,
    KeyCode::KEY_8,
    KeyCode::KEY_9,
    KeyCode::KEY_0,
    KeyCode::KEY_MINUS,
    KeyCode::KEY_EQUAL,
    KeyCode::KEY_BACKSPACE,
    KeyCode::KEY_TAB,
    KeyCode::KEY_Q,
    KeyCode::KEY_W,
    KeyCode::KEY_E,
    KeyCode::KEY_R,
    KeyCode::KEY_T,
    KeyCode::KEY_Y,
    KeyCode::KEY_U,
    KeyCode::KEY_I,
    KeyCode::KEY_O,
    KeyCode::KEY_P,
    KeyCode::KEY_LEFTBRACE,
    KeyCode::KEY_RIGHTBRACE,
    KeyCode::KEY_ENTER,
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_A,
    KeyCode::KEY_S,
    KeyCode::KEY_D,
    KeyCode::KEY_F,
    KeyCode::KEY_G,
    KeyCode::KEY_H,
    KeyCode::KEY_J,
    KeyCode::KEY_K,
    KeyCode::KEY_L,
    KeyCode::KEY_SEMICOLON,
    KeyCode::KEY_APOSTROPHE,
    KeyCode::KEY_GRAVE,
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_BACKSLASH,
    KeyCode::KEY_Z,
    KeyCode::KEY_X,
    KeyCode::KEY_C,
    KeyCode::KEY_V,
    KeyCode::KEY_B,
    KeyCode::KEY_N,
    KeyCode::KEY_M,
    KeyCode::KEY_COMMA,
    KeyCode::KEY_DOT,
    KeyCode::KEY_SLASH,
    KeyCode::KEY_RIGHTSHIFT,
    KeyCode::KEY_KPASTERISK,
    KeyCode::KEY_LEFTALT,
    KeyCode::KEY_SPACE,
    KeyCode::KEY_CAPSLOCK,
    KeyCode::KEY_F1,
    KeyCode::KEY_F2,
    KeyCode::KEY_F3,
    KeyCode::KEY_F4,
    KeyCode::KEY_F5,
    KeyCode::KEY_F6,
    KeyCode::KEY_F7,
    KeyCode::KEY_F8,
    KeyCode::KEY_F9,
    KeyCode::KEY_F10,
    KeyCode::KEY_NUMLOCK,
    KeyCode::KEY_SCROLLLOCK,
    KeyCode::BTN_LEFT,
    KeyCode::BTN_RIGHT,
    KeyCode::BTN_MIDDLE,
    KeyCode::BTN_SIDE,
    KeyCode::BTN_EXTRA,
    KeyCode::BTN_FORWARD,
    KeyCode::BTN_BACK,
    KeyCode::BTN_TASK,
    KeyCode::KEY_KP7,
    KeyCode::KEY_KP8,
    KeyCode::KEY_KP9,
    KeyCode::KEY_KPMINUS,
    KeyCode::KEY_KP4,
    KeyCode::KEY_KP5,
    KeyCode::KEY_KP6,
    KeyCode::KEY_KPPLUS,
    KeyCode::KEY_KP1,
    KeyCode::KEY_KP2,
    KeyCode::KEY_KP3,
    KeyCode::KEY_KP0,
    KeyCode::KEY_KPDOT,
    KeyCode::KEY_ZENKAKUHANKAKU,
    KeyCode::KEY_102ND,
    KeyCode::KEY_F11,
    KeyCode::KEY_F12,
    KeyCode::KEY_RO,
    KeyCode::KEY_KATAKANA,
    KeyCode::KEY_HIRAGANA,
    KeyCode::KEY_HENKAN,
    KeyCode::KEY_KATAKANAHIRAGANA,
    KeyCode::KEY_MUHENKAN,
    KeyCode::KEY_KPJPCOMMA,
    KeyCode::KEY_KPENTER,
    KeyCode::KEY_RIGHTCTRL,
    KeyCode::KEY_KPSLASH,
    KeyCode::KEY_SYSRQ,
    KeyCode::KEY_RIGHTALT,
    KeyCode::KEY_LINEFEED,
    KeyCode::KEY_HOME,
    KeyCode::KEY_UP,
    KeyCode::KEY_PAGEUP,
    KeyCode::KEY_LEFT,
    KeyCode::KEY_RIGHT,
    KeyCode::KEY_END,
    KeyCode::KEY_DOWN,
    KeyCode::KEY_PAGEDOWN,
    KeyCode::KEY_INSERT,
    KeyCode::KEY_DELETE,
    KeyCode::KEY_MACRO,
    KeyCode::KEY_MUTE,
    KeyCode::KEY_VOLUMEDOWN,
    KeyCode::KEY_VOLUMEUP,
    KeyCode::KEY_POWER,
    KeyCode::KEY_KPEQUAL,
    KeyCode::KEY_KPPLUSMINUS,
    KeyCode::KEY_PAUSE,
    KeyCode::KEY_SCALE,
    KeyCode::KEY_KPCOMMA,
    KeyCode::KEY_HANGEUL,
    // KeyCode::KEY_HANGUEL,
    KeyCode::KEY_HANJA,
    KeyCode::KEY_YEN,
    KeyCode::KEY_LEFTMETA,
    KeyCode::KEY_RIGHTMETA,
    KeyCode::KEY_COMPOSE,
    KeyCode::KEY_STOP,
    KeyCode::KEY_AGAIN,
    KeyCode::KEY_PROPS,
    KeyCode::KEY_UNDO,
    KeyCode::KEY_FRONT,
    KeyCode::KEY_COPY,
    KeyCode::KEY_OPEN,
    KeyCode::KEY_PASTE,
    KeyCode::KEY_FIND,
    KeyCode::KEY_CUT,
    KeyCode::KEY_HELP,
    KeyCode::KEY_MENU,
    KeyCode::KEY_CALC,
    KeyCode::KEY_SETUP,
    KeyCode::KEY_SLEEP,
    KeyCode::KEY_WAKEUP,
    KeyCode::KEY_FILE,
    KeyCode::KEY_SENDFILE,
    KeyCode::KEY_DELETEFILE,
    KeyCode::KEY_XFER,
    KeyCode::KEY_PROG1,
    KeyCode::KEY_PROG2,
    KeyCode::KEY_WWW,
    KeyCode::KEY_MSDOS,
    KeyCode::KEY_COFFEE,
    // KeyCode::KEY_SCREENLOCK,
    KeyCode::KEY_ROTATE_DISPLAY,
    KeyCode::KEY_DIRECTION,
    KeyCode::KEY_CYCLEWINDOWS,
    KeyCode::KEY_MAIL,
    KeyCode::KEY_BOOKMARKS,
    KeyCode::KEY_COMPUTER,
    KeyCode::KEY_BACK,
    KeyCode::KEY_FORWARD,
    KeyCode::KEY_CLOSECD,
    KeyCode::KEY_EJECTCD,
    KeyCode::KEY_EJECTCLOSECD,
    KeyCode::KEY_NEXTSONG,
    KeyCode::KEY_PLAYPAUSE,
    KeyCode::KEY_PREVIOUSSONG,
    KeyCode::KEY_STOPCD,
    KeyCode::KEY_RECORD,
    KeyCode::KEY_REWIND,
    KeyCode::KEY_PHONE,
    KeyCode::KEY_ISO,
    KeyCode::KEY_CONFIG,
    KeyCode::KEY_HOMEPAGE,
    KeyCode::KEY_REFRESH,
    KeyCode::KEY_EXIT,
    KeyCode::KEY_MOVE,
    KeyCode::KEY_EDIT,
    KeyCode::KEY_SCROLLUP,
    KeyCode::KEY_SCROLLDOWN,
    KeyCode::KEY_KPLEFTPAREN,
    KeyCode::KEY_KPRIGHTPAREN,
    KeyCode::KEY_NEW,
    KeyCode::KEY_REDO,
    KeyCode::KEY_F13,
    KeyCode::KEY_F14,
    KeyCode::KEY_F15,
    KeyCode::KEY_F16,
    KeyCode::KEY_F17,
    KeyCode::KEY_F18,
    KeyCode::KEY_F19,
    KeyCode::KEY_F20,
    KeyCode::KEY_F21,
    KeyCode::KEY_F22,
    KeyCode::KEY_F23,
    KeyCode::KEY_F24,
    KeyCode::KEY_PLAYCD,
    KeyCode::KEY_PAUSECD,
    KeyCode::KEY_PROG3,
    KeyCode::KEY_PROG4,
    // KeyCode::KEY_ALL_APPLICATIONS,
    KeyCode::KEY_DASHBOARD,
    KeyCode::KEY_SUSPEND,
    KeyCode::KEY_CLOSE,
    KeyCode::KEY_PLAY,
    KeyCode::KEY_FASTFORWARD,
    KeyCode::KEY_BASSBOOST,
    KeyCode::KEY_PRINT,
    KeyCode::KEY_HP,
    KeyCode::KEY_CAMERA,
    KeyCode::KEY_SOUND,
    KeyCode::KEY_QUESTION,
    KeyCode::KEY_EMAIL,
    KeyCode::KEY_CHAT,
    KeyCode::KEY_SEARCH,
    KeyCode::KEY_CONNECT,
    KeyCode::KEY_FINANCE,
    KeyCode::KEY_SPORT,
    KeyCode::KEY_SHOP,
    KeyCode::KEY_ALTERASE,
    KeyCode::KEY_CANCEL,
    KeyCode::KEY_BRIGHTNESSDOWN,
    KeyCode::KEY_BRIGHTNESSUP,
    KeyCode::KEY_MEDIA,
    KeyCode::KEY_SWITCHVIDEOMODE,
    KeyCode::KEY_KBDILLUMTOGGLE,
    KeyCode::KEY_KBDILLUMDOWN,
    KeyCode::KEY_KBDILLUMUP,
    KeyCode::KEY_SEND,
    KeyCode::KEY_REPLY,
    KeyCode::KEY_FORWARDMAIL,
    KeyCode::KEY_SAVE,
    KeyCode::KEY_DOCUMENTS,
    KeyCode::KEY_BATTERY,
    KeyCode::KEY_BLUETOOTH,
    KeyCode::KEY_WLAN,
    KeyCode::KEY_UWB,
    KeyCode::KEY_UNKNOWN,
    KeyCode::KEY_VIDEO_NEXT,
    KeyCode::KEY_VIDEO_PREV,
    KeyCode::KEY_BRIGHTNESS_CYCLE,
    KeyCode::KEY_BRIGHTNESS_AUTO,
    // KeyCode::KEY_BRIGHTNESS_ZERO,
    KeyCode::KEY_DISPLAY_OFF,
    KeyCode::KEY_WWAN,
    // KeyCode::KEY_WIMAX,
    KeyCode::KEY_RFKILL,
    KeyCode::KEY_MICMUTE,
    // KeyCode::BTN_MISC,
    KeyCode::BTN_0,
    KeyCode::BTN_1,
    KeyCode::BTN_2,
    KeyCode::BTN_3,
    KeyCode::BTN_4,
    KeyCode::BTN_5,
    KeyCode::BTN_6,
    KeyCode::BTN_7,
    KeyCode::BTN_8,
    KeyCode::BTN_9,
    // KeyCode::BTN_MOUSE,
    KeyCode::BTN_LEFT,
    KeyCode::BTN_RIGHT,
    KeyCode::BTN_MIDDLE,
    KeyCode::BTN_SIDE,
    KeyCode::BTN_EXTRA,
    KeyCode::BTN_FORWARD,
    KeyCode::BTN_BACK,
    KeyCode::BTN_TASK,
    // KeyCode::BTN_JOYSTICK,
    KeyCode::BTN_TRIGGER,
    KeyCode::BTN_THUMB,
    KeyCode::BTN_THUMB2,
    KeyCode::BTN_TOP,
    KeyCode::BTN_TOP2,
    KeyCode::BTN_PINKIE,
    KeyCode::BTN_BASE,
    KeyCode::BTN_BASE2,
    KeyCode::BTN_BASE3,
    KeyCode::BTN_BASE4,
    KeyCode::BTN_BASE5,
    KeyCode::BTN_BASE6,
    KeyCode::BTN_DEAD,
    // KeyCode::BTN_GAMEPAD,
    KeyCode::BTN_SOUTH,
    // KeyCode::BTN_A,
    KeyCode::BTN_EAST,
    // KeyCode::BTN_B,
    KeyCode::BTN_C,
    KeyCode::BTN_NORTH,
    // KeyCode::BTN_X,
    KeyCode::BTN_WEST,
    // KeyCode::BTN_Y,
    KeyCode::BTN_Z,
    KeyCode::BTN_TL,
    KeyCode::BTN_TR,
    KeyCode::BTN_TL2,
    KeyCode::BTN_TR2,
    KeyCode::BTN_SELECT,
    KeyCode::BTN_START,
    KeyCode::BTN_MODE,
    KeyCode::BTN_THUMBL,
    KeyCode::BTN_THUMBR,
    // KeyCode::BTN_DIGI,
    KeyCode::BTN_TOOL_PEN,
    KeyCode::BTN_TOOL_RUBBER,
    KeyCode::BTN_TOOL_BRUSH,
    KeyCode::BTN_TOOL_PENCIL,
    KeyCode::BTN_TOOL_AIRBRUSH,
    KeyCode::BTN_TOOL_FINGER,
    KeyCode::BTN_TOOL_MOUSE,
    KeyCode::BTN_TOOL_LENS,
    KeyCode::BTN_TOOL_QUINTTAP,
    // KeyCode::BTN_STYLUS3,
    KeyCode::BTN_TOUCH,
    KeyCode::BTN_STYLUS,
    KeyCode::BTN_STYLUS2,
    KeyCode::BTN_TOOL_DOUBLETAP,
    KeyCode::BTN_TOOL_TRIPLETAP,
    KeyCode::BTN_TOOL_QUADTAP,
    // KeyCode::BTN_WHEEL,
    KeyCode::BTN_GEAR_DOWN,
    KeyCode::BTN_GEAR_UP,
    KeyCode::KEY_OK,
    KeyCode::KEY_SELECT,
    KeyCode::KEY_GOTO,
    KeyCode::KEY_CLEAR,
    KeyCode::KEY_POWER2,
    KeyCode::KEY_OPTION,
    KeyCode::KEY_INFO,
    KeyCode::KEY_TIME,
    KeyCode::KEY_VENDOR,
    KeyCode::KEY_ARCHIVE,
    KeyCode::KEY_PROGRAM,
    KeyCode::KEY_CHANNEL,
    KeyCode::KEY_FAVORITES,
    KeyCode::KEY_EPG,
    KeyCode::KEY_PVR,
    KeyCode::KEY_MHP,
    KeyCode::KEY_LANGUAGE,
    KeyCode::KEY_TITLE,
    KeyCode::KEY_SUBTITLE,
    KeyCode::KEY_ANGLE,
    KeyCode::KEY_FULL_SCREEN,
    KeyCode::KEY_ZOOM,
    KeyCode::KEY_MODE,
    KeyCode::KEY_KEYBOARD,
    // KeyCode::KEY_ASPECT_RATIO,
    KeyCode::KEY_SCREEN,
    KeyCode::KEY_PC,
    KeyCode::KEY_TV,
    KeyCode::KEY_TV2,
    KeyCode::KEY_VCR,
    KeyCode::KEY_VCR2,
    KeyCode::KEY_SAT,
    KeyCode::KEY_SAT2,
    KeyCode::KEY_CD,
    KeyCode::KEY_TAPE,
    KeyCode::KEY_RADIO,
    KeyCode::KEY_TUNER,
    KeyCode::KEY_PLAYER,
    KeyCode::KEY_TEXT,
    KeyCode::KEY_DVD,
    KeyCode::KEY_AUX,
    KeyCode::KEY_MP3,
    KeyCode::KEY_AUDIO,
    KeyCode::KEY_VIDEO,
    KeyCode::KEY_DIRECTORY,
    KeyCode::KEY_LIST,
    KeyCode::KEY_MEMO,
    KeyCode::KEY_CALENDAR,
    KeyCode::KEY_RED,
    KeyCode::KEY_GREEN,
    KeyCode::KEY_YELLOW,
    KeyCode::KEY_BLUE,
    KeyCode::KEY_CHANNELUP,
    KeyCode::KEY_CHANNELDOWN,
    KeyCode::KEY_FIRST,
    KeyCode::KEY_LAST,
    KeyCode::KEY_AB,
    KeyCode::KEY_NEXT,
    KeyCode::KEY_RESTART,
    KeyCode::KEY_SLOW,
    KeyCode::KEY_SHUFFLE,
    KeyCode::KEY_BREAK,
    KeyCode::KEY_PREVIOUS,
    KeyCode::KEY_DIGITS,
    KeyCode::KEY_TEEN,
    KeyCode::KEY_TWEN,
    KeyCode::KEY_VIDEOPHONE,
    KeyCode::KEY_GAMES,
    KeyCode::KEY_ZOOMIN,
    KeyCode::KEY_ZOOMOUT,
    KeyCode::KEY_ZOOMRESET,
    KeyCode::KEY_WORDPROCESSOR,
    KeyCode::KEY_EDITOR,
    KeyCode::KEY_SPREADSHEET,
    KeyCode::KEY_GRAPHICSEDITOR,
    KeyCode::KEY_PRESENTATION,
    KeyCode::KEY_DATABASE,
    KeyCode::KEY_NEWS,
    KeyCode::KEY_VOICEMAIL,
    KeyCode::KEY_ADDRESSBOOK,
    KeyCode::KEY_MESSENGER,
    KeyCode::KEY_DISPLAYTOGGLE,
    // KeyCode::KEY_BRIGHTNESS_TOGGLE,
    KeyCode::KEY_SPELLCHECK,
    KeyCode::KEY_LOGOFF,
    KeyCode::KEY_DOLLAR,
    KeyCode::KEY_EURO,
    KeyCode::KEY_FRAMEBACK,
    KeyCode::KEY_FRAMEFORWARD,
    KeyCode::KEY_CONTEXT_MENU,
    KeyCode::KEY_MEDIA_REPEAT,
    KeyCode::KEY_10CHANNELSUP,
    KeyCode::KEY_10CHANNELSDOWN,
    KeyCode::KEY_IMAGES,
    // KeyCode::KEY_NOTIFICATION_CENTER,
    KeyCode::KEY_PICKUP_PHONE,
    KeyCode::KEY_HANGUP_PHONE,
    // KeyCode::KEY_LINK_PHONE,
    KeyCode::KEY_DEL_EOL,
    KeyCode::KEY_DEL_EOS,
    KeyCode::KEY_INS_LINE,
    KeyCode::KEY_DEL_LINE,
    KeyCode::KEY_FN,
    KeyCode::KEY_FN_ESC,
    KeyCode::KEY_FN_F1,
    KeyCode::KEY_FN_F2,
    KeyCode::KEY_FN_F3,
    KeyCode::KEY_FN_F4,
    KeyCode::KEY_FN_F5,
    KeyCode::KEY_FN_F6,
    KeyCode::KEY_FN_F7,
    KeyCode::KEY_FN_F8,
    KeyCode::KEY_FN_F9,
    KeyCode::KEY_FN_F10,
    KeyCode::KEY_FN_F11,
    KeyCode::KEY_FN_F12,
    KeyCode::KEY_FN_1,
    KeyCode::KEY_FN_2,
    KeyCode::KEY_FN_D,
    KeyCode::KEY_FN_E,
    KeyCode::KEY_FN_F,
    KeyCode::KEY_FN_S,
    KeyCode::KEY_FN_B,
    // KeyCode::KEY_FN_RIGHT_SHIFT,
    KeyCode::KEY_BRL_DOT1,
    KeyCode::KEY_BRL_DOT2,
    KeyCode::KEY_BRL_DOT3,
    KeyCode::KEY_BRL_DOT4,
    KeyCode::KEY_BRL_DOT5,
    KeyCode::KEY_BRL_DOT6,
    KeyCode::KEY_BRL_DOT7,
    KeyCode::KEY_BRL_DOT8,
    KeyCode::KEY_BRL_DOT9,
    KeyCode::KEY_BRL_DOT10,
    KeyCode::KEY_NUMERIC_0,
    KeyCode::KEY_NUMERIC_1,
    KeyCode::KEY_NUMERIC_2,
    KeyCode::KEY_NUMERIC_3,
    KeyCode::KEY_NUMERIC_4,
    KeyCode::KEY_NUMERIC_5,
    KeyCode::KEY_NUMERIC_6,
    KeyCode::KEY_NUMERIC_7,
    KeyCode::KEY_NUMERIC_8,
    KeyCode::KEY_NUMERIC_9,
    KeyCode::KEY_NUMERIC_STAR,
    KeyCode::KEY_NUMERIC_POUND,
    KeyCode::KEY_NUMERIC_A,
    KeyCode::KEY_NUMERIC_B,
    KeyCode::KEY_NUMERIC_C,
    KeyCode::KEY_NUMERIC_D,
    KeyCode::KEY_CAMERA_FOCUS,
    KeyCode::KEY_WPS_BUTTON,
    KeyCode::KEY_TOUCHPAD_TOGGLE,
    KeyCode::KEY_TOUCHPAD_ON,
    KeyCode::KEY_TOUCHPAD_OFF,
    KeyCode::KEY_CAMERA_ZOOMIN,
    KeyCode::KEY_CAMERA_ZOOMOUT,
    KeyCode::KEY_CAMERA_UP,
    KeyCode::KEY_CAMERA_DOWN,
    KeyCode::KEY_CAMERA_LEFT,
    KeyCode::KEY_CAMERA_RIGHT,
    KeyCode::KEY_ATTENDANT_ON,
    KeyCode::KEY_ATTENDANT_OFF,
    KeyCode::KEY_ATTENDANT_TOGGLE,
    KeyCode::KEY_LIGHTS_TOGGLE,
    KeyCode::BTN_DPAD_UP,
    KeyCode::BTN_DPAD_DOWN,
    KeyCode::BTN_DPAD_LEFT,
    KeyCode::BTN_DPAD_RIGHT,
    KeyCode::KEY_ALS_TOGGLE,
    // KeyCode::KEY_ROTATE_LOCK_TOGGLE,
    // KeyCode::KEY_REFRESH_RATE_TOGGLE,
    KeyCode::KEY_BUTTONCONFIG,
    KeyCode::KEY_TASKMANAGER,
    KeyCode::KEY_JOURNAL,
    KeyCode::KEY_CONTROLPANEL,
    KeyCode::KEY_APPSELECT,
    KeyCode::KEY_SCREENSAVER,
    KeyCode::KEY_VOICECOMMAND,
    KeyCode::KEY_ASSISTANT,
    KeyCode::KEY_KBD_LAYOUT_NEXT,
    // KeyCode::KEY_EMOJI_PICKER,
    // KeyCode::KEY_DICTATE,
    // KeyCode::KEY_CAMERA_ACCESS_ENABLE,
    // KeyCode::KEY_CAMERA_ACCESS_DISABLE,
    // KeyCode::KEY_CAMERA_ACCESS_TOGGLE,
    // KeyCode::KEY_ACCESSIBILITY,
    // KeyCode::KEY_DO_NOT_DISTURB,
    KeyCode::KEY_BRIGHTNESS_MIN,
    KeyCode::KEY_BRIGHTNESS_MAX,
    KeyCode::KEY_KBDINPUTASSIST_PREV,
    KeyCode::KEY_KBDINPUTASSIST_NEXT,
    KeyCode::KEY_KBDINPUTASSIST_PREVGROUP,
    KeyCode::KEY_KBDINPUTASSIST_NEXTGROUP,
    KeyCode::KEY_KBDINPUTASSIST_ACCEPT,
    KeyCode::KEY_KBDINPUTASSIST_CANCEL,
    KeyCode::KEY_RIGHT_UP,
    KeyCode::KEY_RIGHT_DOWN,
    KeyCode::KEY_LEFT_UP,
    KeyCode::KEY_LEFT_DOWN,
    KeyCode::KEY_ROOT_MENU,
    KeyCode::KEY_MEDIA_TOP_MENU,
    KeyCode::KEY_NUMERIC_11,
    KeyCode::KEY_NUMERIC_12,
    KeyCode::KEY_AUDIO_DESC,
    KeyCode::KEY_3D_MODE,
    KeyCode::KEY_NEXT_FAVORITE,
    KeyCode::KEY_STOP_RECORD,
    KeyCode::KEY_PAUSE_RECORD,
    KeyCode::KEY_VOD,
    KeyCode::KEY_UNMUTE,
    KeyCode::KEY_FASTREVERSE,
    KeyCode::KEY_SLOWREVERSE,
    KeyCode::KEY_DATA,
    KeyCode::KEY_ONSCREEN_KEYBOARD,
    KeyCode::KEY_PRIVACY_SCREEN_TOGGLE,
    KeyCode::KEY_SELECTIVE_SCREENSHOT,
    // KeyCode::KEY_NEXT_ELEMENT,
    // KeyCode::KEY_PREVIOUS_ELEMENT,
    // KeyCode::KEY_AUTOPILOT_ENGAGE_TOGGLE,
    // KeyCode::KEY_MARK_WAYPOINT,
    // KeyCode::KEY_SOS,
    // KeyCode::KEY_NAV_CHART,
    // KeyCode::KEY_FISHING_CHART,
    // KeyCode::KEY_SINGLE_RANGE_RADAR,
    // KeyCode::KEY_DUAL_RANGE_RADAR,
    // KeyCode::KEY_RADAR_OVERLAY,
    // KeyCode::KEY_TRADITIONAL_SONAR,
    // KeyCode::KEY_CLEARVU_SONAR,
    // KeyCode::KEY_SIDEVU_SONAR,
    // KeyCode::KEY_NAV_INFO,
    // KeyCode::KEY_BRIGHTNESS_MENU,
    // KeyCode::KEY_MACRO1,
    // KeyCode::KEY_MACRO2,
    // KeyCode::KEY_MACRO3,
    // KeyCode::KEY_MACRO4,
    // KeyCode::KEY_MACRO5,
    // KeyCode::KEY_MACRO6,
    // KeyCode::KEY_MACRO7,
    // KeyCode::KEY_MACRO8,
    // KeyCode::KEY_MACRO9,
    // KeyCode::KEY_MACRO10,
    // KeyCode::KEY_MACRO11,
    // KeyCode::KEY_MACRO12,
    // KeyCode::KEY_MACRO13,
    // KeyCode::KEY_MACRO14,
    // KeyCode::KEY_MACRO15,
    // KeyCode::KEY_MACRO16,
    // KeyCode::KEY_MACRO17,
    // KeyCode::KEY_MACRO18,
    // KeyCode::KEY_MACRO19,
    // KeyCode::KEY_MACRO20,
    // KeyCode::KEY_MACRO21,
    // KeyCode::KEY_MACRO22,
    // KeyCode::KEY_MACRO23,
    // KeyCode::KEY_MACRO24,
    // KeyCode::KEY_MACRO25,
    // KeyCode::KEY_MACRO26,
    // KeyCode::KEY_MACRO27,
    // KeyCode::KEY_MACRO28,
    // KeyCode::KEY_MACRO29,
    // KeyCode::KEY_MACRO30,
    // KeyCode::KEY_MACRO_RECORD_START,
    // KeyCode::KEY_MACRO_RECORD_STOP,
    // KeyCode::KEY_MACRO_PRESET_CYCLE,
    // KeyCode::KEY_MACRO_PRESET1,
    // KeyCode::KEY_MACRO_PRESET2,
    // KeyCode::KEY_MACRO_PRESET3,
    // KeyCode::KEY_KBD_LCD_MENU1,
    // KeyCode::KEY_KBD_LCD_MENU2,
    // KeyCode::KEY_KBD_LCD_MENU3,
    // KeyCode::KEY_KBD_LCD_MENU4,
    // KeyCode::KEY_KBD_LCD_MENU5,
    // KeyCode::BTN_TRIGGER_HAPPY,
    KeyCode::BTN_TRIGGER_HAPPY1,
    KeyCode::BTN_TRIGGER_HAPPY2,
    KeyCode::BTN_TRIGGER_HAPPY3,
    KeyCode::BTN_TRIGGER_HAPPY4,
    KeyCode::BTN_TRIGGER_HAPPY5,
    KeyCode::BTN_TRIGGER_HAPPY6,
    KeyCode::BTN_TRIGGER_HAPPY7,
    KeyCode::BTN_TRIGGER_HAPPY8,
    KeyCode::BTN_TRIGGER_HAPPY9,
    KeyCode::BTN_TRIGGER_HAPPY10,
    KeyCode::BTN_TRIGGER_HAPPY11,
    KeyCode::BTN_TRIGGER_HAPPY12,
    KeyCode::BTN_TRIGGER_HAPPY13,
    KeyCode::BTN_TRIGGER_HAPPY14,
    KeyCode::BTN_TRIGGER_HAPPY15,
    KeyCode::BTN_TRIGGER_HAPPY16,
    KeyCode::BTN_TRIGGER_HAPPY17,
    KeyCode::BTN_TRIGGER_HAPPY18,
    KeyCode::BTN_TRIGGER_HAPPY19,
    KeyCode::BTN_TRIGGER_HAPPY20,
    KeyCode::BTN_TRIGGER_HAPPY21,
    KeyCode::BTN_TRIGGER_HAPPY22,
    KeyCode::BTN_TRIGGER_HAPPY23,
    KeyCode::BTN_TRIGGER_HAPPY24,
    KeyCode::BTN_TRIGGER_HAPPY25,
    KeyCode::BTN_TRIGGER_HAPPY26,
    KeyCode::BTN_TRIGGER_HAPPY27,
    KeyCode::BTN_TRIGGER_HAPPY28,
    KeyCode::BTN_TRIGGER_HAPPY29,
    KeyCode::BTN_TRIGGER_HAPPY30,
    KeyCode::BTN_TRIGGER_HAPPY31,
    KeyCode::BTN_TRIGGER_HAPPY32,
    KeyCode::BTN_TRIGGER_HAPPY33,
    KeyCode::BTN_TRIGGER_HAPPY34,
    KeyCode::BTN_TRIGGER_HAPPY35,
    KeyCode::BTN_TRIGGER_HAPPY36,
    KeyCode::BTN_TRIGGER_HAPPY37,
    KeyCode::BTN_TRIGGER_HAPPY38,
    KeyCode::BTN_TRIGGER_HAPPY39,
    KeyCode::BTN_TRIGGER_HAPPY40,
];
