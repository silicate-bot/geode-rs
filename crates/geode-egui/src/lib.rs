#![allow(unsafe_op_in_unsafe_fn)]

use std::cell::RefCell;
use std::ffi::c_char;
use std::fmt;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, Rect, Vec2};
use egui_glow::{Painter, glow};
use glow::HasContext as _;

mod platform;

pub use egui;
pub use geode_rs;

type UiCallback = dyn FnMut(&egui::Context);

const TOUCH_BEGAN: u32 = 0;
const TOUCH_MOVED: u32 = 1;
const TOUCH_ENDED: u32 = 2;
const TOUCH_CANCELLED: u32 = 3;
const SCROLL_MULTIPLIER: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Default,
    Blocking,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    MissingDirector,
    MissingScreenSize,
    GlowContextUnavailable,
    PainterInit(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDirector => f.write_str("cocos director is not available yet"),
            Self::MissingScreenSize => f.write_str("cocos screen size is not available yet"),
            Self::GlowContextUnavailable => {
                f.write_str("failed to load OpenGL symbols from the active cocos context")
            }
            Self::PainterInit(error) => {
                write!(f, "failed to initialize egui_glow painter: {error}")
            }
        }
    }
}

impl std::error::Error for RenderError {}

#[derive(Clone, Copy)]
struct FrameInfo {
    points: Vec2,
    egui_size: Vec2,
    pixels: [u32; 2],
    pixels_per_point: f32,
    delta_time: f32,
}

struct GlStateSnapshot {
    viewport: [i32; 4],
    scissor_box: [i32; 4],
    scissor_test: bool,
    cull_face: bool,
    depth_test: bool,
    blend: bool,
    current_program: Option<glow::Program>,
    array_buffer: Option<glow::Buffer>,
    element_array_buffer: Option<glow::Buffer>,
    vertex_array: Option<glow::VertexArray>,
    active_texture: u32,
    texture_2d: Option<glow::Texture>,
    unpack_alignment: i32,
    blend_src_rgb: u32,
    blend_dst_rgb: u32,
    blend_src_alpha: u32,
    blend_dst_alpha: u32,
    blend_equation_rgb: u32,
    blend_equation_alpha: u32,
}

impl GlStateSnapshot {
    fn capture(gl: &glow::Context) -> Self {
        unsafe {
            let mut viewport = [0; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);

            let mut scissor_box = [0; 4];
            gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut scissor_box);

            Self {
                viewport,
                scissor_box,
                scissor_test: gl.is_enabled(glow::SCISSOR_TEST),
                cull_face: gl.is_enabled(glow::CULL_FACE),
                depth_test: gl.is_enabled(glow::DEPTH_TEST),
                blend: gl.is_enabled(glow::BLEND),
                current_program: gl.get_parameter_program(glow::CURRENT_PROGRAM),
                array_buffer: gl.get_parameter_buffer(glow::ARRAY_BUFFER_BINDING),
                element_array_buffer: gl.get_parameter_buffer(glow::ELEMENT_ARRAY_BUFFER_BINDING),
                vertex_array: gl.get_parameter_vertex_array(glow::VERTEX_ARRAY_BINDING),
                active_texture: gl.get_parameter_i32(glow::ACTIVE_TEXTURE) as u32,
                texture_2d: gl.get_parameter_texture(glow::TEXTURE_BINDING_2D),
                unpack_alignment: gl.get_parameter_i32(glow::UNPACK_ALIGNMENT),
                blend_src_rgb: gl.get_parameter_i32(glow::BLEND_SRC_RGB) as u32,
                blend_dst_rgb: gl.get_parameter_i32(glow::BLEND_DST_RGB) as u32,
                blend_src_alpha: gl.get_parameter_i32(glow::BLEND_SRC_ALPHA) as u32,
                blend_dst_alpha: gl.get_parameter_i32(glow::BLEND_DST_ALPHA) as u32,
                blend_equation_rgb: gl.get_parameter_i32(glow::BLEND_EQUATION_RGB) as u32,
                blend_equation_alpha: gl.get_parameter_i32(glow::BLEND_EQUATION_ALPHA) as u32,
            }
        }
    }

    fn restore(&self, gl: &glow::Context) {
        unsafe {
            set_gl_flag(gl, glow::SCISSOR_TEST, self.scissor_test);
            set_gl_flag(gl, glow::CULL_FACE, self.cull_face);
            set_gl_flag(gl, glow::DEPTH_TEST, self.depth_test);
            set_gl_flag(gl, glow::BLEND, self.blend);

            gl.use_program(self.current_program);
            gl.bind_buffer(glow::ARRAY_BUFFER, self.array_buffer);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, self.element_array_buffer);
            gl.bind_vertex_array(self.vertex_array);

            gl.active_texture(self.active_texture);
            gl.bind_texture(glow::TEXTURE_2D, self.texture_2d);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, self.unpack_alignment);

            gl.blend_equation_separate(self.blend_equation_rgb, self.blend_equation_alpha);
            gl.blend_func_separate(
                self.blend_src_rgb,
                self.blend_dst_rgb,
                self.blend_src_alpha,
                self.blend_dst_alpha,
            );

            gl.viewport(
                self.viewport[0],
                self.viewport[1],
                self.viewport[2],
                self.viewport[3],
            );
            gl.scissor(
                self.scissor_box[0],
                self.scissor_box[1],
                self.scissor_box[2],
                self.scissor_box[3],
            );
        }
    }
}

fn set_gl_flag(gl: &glow::Context, flag: u32, enabled: bool) {
    unsafe {
        if enabled {
            gl.enable(flag);
        } else {
            gl.disable(flag);
        }
    }
}

struct Backend {
    ctx: egui::Context,
    gl: Option<Arc<glow::Context>>,
    painter: Option<Painter>,
    callback: Option<Box<UiCallback>>,
    time_seconds: f64,
    last_error: Option<String>,
}

struct InputState {
    pending_events: Vec<Event>,
    modifiers: Modifiers,
    visible: bool,
    input_mode: InputMode,
    wants_pointer_input: bool,
    wants_keyboard_input: bool,
}

impl Default for Backend {
    fn default() -> Self {
        Self {
            ctx: egui::Context::default(),
            gl: None,
            painter: None,
            callback: None,
            time_seconds: 0.0,
            last_error: None,
        }
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            pending_events: Vec::new(),
            modifiers: Modifiers::default(),
            visible: true,
            input_mode: InputMode::Default,
            wants_pointer_input: false,
            wants_keyboard_input: false,
        }
    }
}

impl Backend {
    fn destroy_painter(&mut self) {
        if let Some(mut painter) = self.painter.take() {
            painter.destroy();
        }
        self.gl = None;
    }

    fn ensure_painter(&mut self) -> Result<(), RenderError> {
        if self.painter.is_some() {
            return Ok(());
        }

        let gl = make_glow_context()?;
        let painter = Painter::new(gl.clone(), "", None, false)
            .map_err(|error| RenderError::PainterInit(error.to_string()))?;

        self.gl = Some(gl);
        self.painter = Some(painter);
        Ok(())
    }

    fn queue_platform_pointer_event(&mut self, frame: FrameInfo, input: &mut InputState) {
        if let Some(position) = current_desktop_pointer_position(frame) {
            input.pending_events.push(Event::PointerMoved(position));
        }
    }

    fn try_paint_frame(&mut self) -> Result<(), RenderError> {
        let mut input = lock_input_state();
        if !input.visible {
            input.pending_events.clear();
            input.wants_pointer_input = false;
            input.wants_keyboard_input = false;
            return Ok(());
        }

        let mut frame = current_frame_info()?;
        self.ensure_painter()?;
        if let Some(gl) = self.gl.as_ref() {
            let mut viewport = [0; 4];
            unsafe {
                gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
            }
            if viewport[2] > 0 && viewport[3] > 0 {
                frame.egui_size = Vec2::new(viewport[2] as f32, viewport[3] as f32);
                frame.pixels = [viewport[2] as u32, viewport[3] as u32];
            }
        }
        update_last_frame_info(frame);
        self.queue_platform_pointer_event(frame, &mut input);

        let delta_time = if frame.delta_time > 0.0 {
            frame.delta_time
        } else {
            1.0 / 60.0
        };
        self.time_seconds += delta_time as f64;

        let mut raw_input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, frame.egui_size)),
            time: Some(self.time_seconds),
            predicted_dt: delta_time,
            modifiers: input.modifiers,
            events: mem::take(&mut input.pending_events),
            focused: true,
            ..Default::default()
        };
        drop(input);
        let viewport = raw_input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default();
        viewport.native_pixels_per_point = Some(frame.pixels_per_point);
        viewport.inner_rect = Some(Rect::from_min_size(Pos2::ZERO, frame.egui_size));

        let mut callback = self.callback.as_deref_mut();
        let full_output = self.ctx.run(raw_input, |ctx| {
            if let Some(callback) = callback.as_deref_mut() {
                callback(ctx);
            }
        });
        {
            let mut input = lock_input_state();
            input.wants_pointer_input = self.ctx.wants_pointer_input();
            input.wants_keyboard_input = self.ctx.wants_keyboard_input();
        }
        let clipped_primitives = self
            .ctx
            .tessellate(full_output.shapes, frame.pixels_per_point);

        if let Some(painter) = self.painter.as_mut() {
            let gl = self
                .gl
                .as_ref()
                .expect("glow context should exist whenever painter exists");
            let state = GlStateSnapshot::capture(gl);

            painter.paint_and_update_textures(
                frame.pixels,
                frame.pixels_per_point,
                &clipped_primitives,
                &full_output.textures_delta,
            );

            state.restore(gl);
        }

        Ok(())
    }
}

static INPUT_STATE: OnceLock<Mutex<InputState>> = OnceLock::new();
static LAST_FRAME_INFO: OnceLock<Mutex<Option<FrameInfo>>> = OnceLock::new();

thread_local! {
    static BACKEND: RefCell<Backend> = RefCell::new(Backend::default());
}

fn lock_input_state() -> MutexGuard<'static, InputState> {
    INPUT_STATE
        .get_or_init(|| Mutex::new(InputState::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn with_input_state<R>(f: impl FnOnce(&mut InputState) -> R) -> R {
    let mut input = lock_input_state();
    f(&mut input)
}

fn with_backend<R>(f: impl FnOnce(&mut Backend) -> R) -> R {
    BACKEND.with(|backend| f(&mut backend.borrow_mut()))
}

fn update_last_frame_info(frame: FrameInfo) {
    let mut last = LAST_FRAME_INFO
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *last = Some(frame);
}

fn last_frame_info() -> Option<FrameInfo> {
    *LAST_FRAME_INFO
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn current_frame_info() -> Result<FrameInfo, RenderError> {
    unsafe {
        let director = geode_rs::cocos::CCDirector::sharedDirector();
        if director.is_null() {
            return Err(RenderError::MissingDirector);
        }

        let win_size = (*director).getWinSize();
        let delta_time = geode_rs::cocos::CCDirector_getDeltaTime(director.cast());

        let points = Vec2::new(win_size.width.max(1.0), win_size.height.max(1.0));
        let win_size_pixels = (*director).getWinSizeInPixels();
        let mut pixels = [
            win_size_pixels.width.max(1.0).round() as u32,
            win_size_pixels.height.max(1.0).round() as u32,
        ];

        if pixels[0] == 0 || pixels[1] == 0 {
            let view = geode_rs::cocos::CCEGLView::sharedOpenGLView();
            if !view.is_null() {
                let frame_size = geode_rs::cocos::CCEGLViewProtocol_getFrameSize(view.cast());
                if !frame_size.is_null() {
                    let factor = geode_rs::geode_display_factor().max(1.0);
                    pixels = [
                        ((*frame_size).width * factor).max(1.0).round() as u32,
                        ((*frame_size).height * factor).max(1.0).round() as u32,
                    ];
                }
            }
        }

        if pixels[0] == 0 || pixels[1] == 0 {
            return Err(RenderError::MissingScreenSize);
        }

        let egui_size = Vec2::new(pixels[0] as f32, pixels[1] as f32);

        Ok(FrameInfo {
            points,
            egui_size,
            pixels,
            pixels_per_point: 1.0,
            delta_time,
        })
    }
}

#[cfg(not(target_os = "android"))]
fn current_desktop_pointer_position(frame: FrameInfo) -> Option<Pos2> {
    cocos_point_to_egui_with_frame(geode_rs::geode_mouse_position(), frame)
}

#[cfg(target_os = "android")]
fn current_desktop_pointer_position(_frame: FrameInfo) -> Option<Pos2> {
    None
}

fn make_glow_context() -> Result<Arc<glow::Context>, RenderError> {
    catch_unwind(AssertUnwindSafe(|| unsafe {
        Arc::new(glow::Context::from_loader_function_cstr(
            platform::load_gl_symbol,
        ))
    }))
    .map_err(|_| RenderError::GlowContextUnavailable)
}

fn update_last_error(backend: &mut Backend, result: &Result<(), RenderError>) {
    match result {
        Ok(()) => backend.last_error = None,
        Err(error) => {
            let message = error.to_string();
            if backend.last_error.as_deref() != Some(message.as_str()) {
                log::warn!("geode-egui: {message}");
            }
            backend.last_error = Some(message);
        }
    }
}

fn current_modifiers() -> Modifiers {
    lock_input_state().modifiers
}

fn set_modifiers_internal(modifiers: Modifiers) {
    with_input_state(|input| input.modifiers = modifiers);
}

fn current_input_mode() -> InputMode {
    lock_input_state().input_mode
}

fn is_visible_internal() -> bool {
    lock_input_state().visible
}

fn should_block_input() -> bool {
    is_visible_internal() && current_input_mode() == InputMode::Blocking
}

fn should_capture_pointer() -> bool {
    should_block_input() || wants_pointer_input()
}

fn should_capture_keyboard() -> bool {
    should_block_input() || wants_keyboard_input()
}


fn update_modifiers_from_key_event(key: geode_rs::cocos::enumKeyCodes, pressed: bool) {
    with_input_state(|input| {
        let mut modifiers = input.modifiers;
        match key {
            geode_rs::cocos::enumKeyCodes_KEY_Shift
            | geode_rs::cocos::enumKeyCodes_KEY_LeftShift
            | geode_rs::cocos::enumKeyCodes_KEY_RightShift => modifiers.shift = pressed,
            geode_rs::cocos::enumKeyCodes_KEY_Control
            | geode_rs::cocos::enumKeyCodes_KEY_LeftControl
            | geode_rs::cocos::enumKeyCodes_KEY_RightControl => modifiers.ctrl = pressed,
            geode_rs::cocos::enumKeyCodes_KEY_Alt
            | geode_rs::cocos::enumKeyCodes_KEY_LeftMenu
            | geode_rs::cocos::enumKeyCodes_KEY_RightMenu => modifiers.alt = pressed,
            geode_rs::cocos::enumKeyCodes_KEY_LeftWindowsKey
            | geode_rs::cocos::enumKeyCodes_KEY_RightWindowsKey => {
                if cfg!(any(target_os = "macos", target_os = "ios")) {
                    modifiers.mac_cmd = pressed;
                }
            }
            _ => {}
        }

        modifiers.command = if cfg!(any(target_os = "macos", target_os = "ios")) {
            modifiers.mac_cmd
        } else {
            modifiers.ctrl
        };
        input.modifiers = modifiers;
    });
}

fn cocos_key_to_egui_key(key: geode_rs::cocos::enumKeyCodes) -> Option<Key> {
    match key {
        geode_rs::cocos::enumKeyCodes_KEY_Up => Some(Key::ArrowUp),
        geode_rs::cocos::enumKeyCodes_KEY_Down => Some(Key::ArrowDown),
        geode_rs::cocos::enumKeyCodes_KEY_Left => Some(Key::ArrowLeft),
        geode_rs::cocos::enumKeyCodes_KEY_Right => Some(Key::ArrowRight),
        geode_rs::cocos::enumKeyCodes_KEY_ArrowUp => Some(Key::ArrowUp),
        geode_rs::cocos::enumKeyCodes_KEY_ArrowDown => Some(Key::ArrowDown),
        geode_rs::cocos::enumKeyCodes_KEY_ArrowLeft => Some(Key::ArrowLeft),
        geode_rs::cocos::enumKeyCodes_KEY_ArrowRight => Some(Key::ArrowRight),
        geode_rs::cocos::enumKeyCodes_KEY_Escape => Some(Key::Escape),
        geode_rs::cocos::enumKeyCodes_KEY_Tab => Some(Key::Tab),
        geode_rs::cocos::enumKeyCodes_KEY_Backspace => Some(Key::Backspace),
        geode_rs::cocos::enumKeyCodes_KEY_Enter => Some(Key::Enter),
        geode_rs::cocos::enumKeyCodes_KEY_Space => Some(Key::Space),
        geode_rs::cocos::enumKeyCodes_KEY_Insert => Some(Key::Insert),
        geode_rs::cocos::enumKeyCodes_KEY_Delete => Some(Key::Delete),
        geode_rs::cocos::enumKeyCodes_KEY_Home => Some(Key::Home),
        geode_rs::cocos::enumKeyCodes_KEY_End => Some(Key::End),
        geode_rs::cocos::enumKeyCodes_KEY_PageUp => Some(Key::PageUp),
        geode_rs::cocos::enumKeyCodes_KEY_PageDown => Some(Key::PageDown),
        geode_rs::cocos::enumKeyCodes_KEY_Apostrophe => Some(Key::Quote),
        geode_rs::cocos::enumKeyCodes_KEY_OEMComma => Some(Key::Comma),
        geode_rs::cocos::enumKeyCodes_KEY_OEMMinus => Some(Key::Minus),
        geode_rs::cocos::enumKeyCodes_KEY_OEMPeriod => Some(Key::Period),
        geode_rs::cocos::enumKeyCodes_KEY_OEMPlus => Some(Key::Plus),
        geode_rs::cocos::enumKeyCodes_KEY_Semicolon => Some(Key::Semicolon),
        geode_rs::cocos::enumKeyCodes_KEY_Slash => Some(Key::Slash),
        geode_rs::cocos::enumKeyCodes_KEY_GraveAccent => Some(Key::Backtick),
        geode_rs::cocos::enumKeyCodes_KEY_LeftBracket => Some(Key::OpenBracket),
        geode_rs::cocos::enumKeyCodes_KEY_Backslash => Some(Key::Backslash),
        geode_rs::cocos::enumKeyCodes_KEY_RightBracket => Some(Key::CloseBracket),
        geode_rs::cocos::enumKeyCodes_KEY_Zero => Some(Key::Num0),
        geode_rs::cocos::enumKeyCodes_KEY_One => Some(Key::Num1),
        geode_rs::cocos::enumKeyCodes_KEY_Two => Some(Key::Num2),
        geode_rs::cocos::enumKeyCodes_KEY_Three => Some(Key::Num3),
        geode_rs::cocos::enumKeyCodes_KEY_Four => Some(Key::Num4),
        geode_rs::cocos::enumKeyCodes_KEY_Five => Some(Key::Num5),
        geode_rs::cocos::enumKeyCodes_KEY_Six => Some(Key::Num6),
        geode_rs::cocos::enumKeyCodes_KEY_Seven => Some(Key::Num7),
        geode_rs::cocos::enumKeyCodes_KEY_Eight => Some(Key::Num8),
        geode_rs::cocos::enumKeyCodes_KEY_Nine => Some(Key::Num9),
        geode_rs::cocos::enumKeyCodes_KEY_A => Some(Key::A),
        geode_rs::cocos::enumKeyCodes_KEY_B => Some(Key::B),
        geode_rs::cocos::enumKeyCodes_KEY_C => Some(Key::C),
        geode_rs::cocos::enumKeyCodes_KEY_D => Some(Key::D),
        geode_rs::cocos::enumKeyCodes_KEY_E => Some(Key::E),
        geode_rs::cocos::enumKeyCodes_KEY_F => Some(Key::F),
        geode_rs::cocos::enumKeyCodes_KEY_G => Some(Key::G),
        geode_rs::cocos::enumKeyCodes_KEY_H => Some(Key::H),
        geode_rs::cocos::enumKeyCodes_KEY_I => Some(Key::I),
        geode_rs::cocos::enumKeyCodes_KEY_J => Some(Key::J),
        geode_rs::cocos::enumKeyCodes_KEY_K => Some(Key::K),
        geode_rs::cocos::enumKeyCodes_KEY_L => Some(Key::L),
        geode_rs::cocos::enumKeyCodes_KEY_M => Some(Key::M),
        geode_rs::cocos::enumKeyCodes_KEY_N => Some(Key::N),
        geode_rs::cocos::enumKeyCodes_KEY_O => Some(Key::O),
        geode_rs::cocos::enumKeyCodes_KEY_P => Some(Key::P),
        geode_rs::cocos::enumKeyCodes_KEY_Q => Some(Key::Q),
        geode_rs::cocos::enumKeyCodes_KEY_R => Some(Key::R),
        geode_rs::cocos::enumKeyCodes_KEY_S => Some(Key::S),
        geode_rs::cocos::enumKeyCodes_KEY_T => Some(Key::T),
        geode_rs::cocos::enumKeyCodes_KEY_U => Some(Key::U),
        geode_rs::cocos::enumKeyCodes_KEY_V => Some(Key::V),
        geode_rs::cocos::enumKeyCodes_KEY_W => Some(Key::W),
        geode_rs::cocos::enumKeyCodes_KEY_X => Some(Key::X),
        geode_rs::cocos::enumKeyCodes_KEY_Y => Some(Key::Y),
        geode_rs::cocos::enumKeyCodes_KEY_Z => Some(Key::Z),
        geode_rs::cocos::enumKeyCodes_KEY_F1 => Some(Key::F1),
        geode_rs::cocos::enumKeyCodes_KEY_F2 => Some(Key::F2),
        geode_rs::cocos::enumKeyCodes_KEY_F3 => Some(Key::F3),
        geode_rs::cocos::enumKeyCodes_KEY_F4 => Some(Key::F4),
        geode_rs::cocos::enumKeyCodes_KEY_F5 => Some(Key::F5),
        geode_rs::cocos::enumKeyCodes_KEY_F6 => Some(Key::F6),
        geode_rs::cocos::enumKeyCodes_KEY_F7 => Some(Key::F7),
        geode_rs::cocos::enumKeyCodes_KEY_F8 => Some(Key::F8),
        geode_rs::cocos::enumKeyCodes_KEY_F9 => Some(Key::F9),
        geode_rs::cocos::enumKeyCodes_KEY_F10 => Some(Key::F10),
        geode_rs::cocos::enumKeyCodes_KEY_F11 => Some(Key::F11),
        geode_rs::cocos::enumKeyCodes_KEY_F12 => Some(Key::F12),
        _ => None,
    }
}

pub fn context() -> egui::Context {
    BACKEND.with(|backend| backend.borrow().ctx.clone())
}

pub fn set_ui<F>(callback: F)
where
    F: FnMut(&egui::Context) + 'static,
{
    with_backend(|backend| backend.callback = Some(Box::new(callback)));
}

pub fn clear_ui() {
    with_backend(|backend| backend.callback = None);
}

pub fn set_visible(visible: bool) {
    with_input_state(|input| {
        input.visible = visible;
        if !visible {
            input.pending_events.clear();
            input.wants_pointer_input = false;
            input.wants_keyboard_input = false;
        }
    });
}

pub fn toggle_visibility() {
    with_input_state(|input| input.visible = !input.visible);
}

pub fn is_visible() -> bool {
    is_visible_internal()
}

pub fn set_input_mode(input_mode: InputMode) {
    with_input_state(|input| input.input_mode = input_mode);
}

pub fn input_mode() -> InputMode {
    current_input_mode()
}

pub fn set_modifiers(modifiers: Modifiers) {
    set_modifiers_internal(modifiers);
}

pub fn modifiers() -> Modifiers {
    current_modifiers()
}

pub fn push_event(event: Event) {
    with_input_state(|input| input.pending_events.push(event));
}

pub fn push_text(text: impl Into<String>) {
    push_event(Event::Text(text.into()));
}

pub fn push_key(key: Key, pressed: bool, repeat: bool) {
    push_event(Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat,
        modifiers: modifiers(),
    });
}

pub fn push_scroll(delta: Vec2) {
    push_event(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta,
        modifiers: modifiers(),
    });
}

pub fn cocos_point_to_egui(point: geode_rs::cocos::CCPoint) -> Option<Pos2> {
    let frame = last_frame_info().or_else(|| current_frame_info().ok())?;
    cocos_point_to_egui_with_frame(point, frame)
}

pub fn frame_pixels_to_egui(position: Pos2) -> Option<Pos2> {
    Some(position)
}

fn cocos_point_to_egui_with_frame(
    point: geode_rs::cocos::CCPoint,
    frame: FrameInfo,
) -> Option<Pos2> {
    if point.x == 0.0 && point.y == 0.0 {
        return None;
    }

    Some(Pos2::new(
        point.x / frame.points.x * frame.egui_size.x,
        (1.0 - point.y / frame.points.y) * frame.egui_size.y,
    ))
}

pub fn push_cocos_pointer_moved(point: geode_rs::cocos::CCPoint) {
    if let Some(position) = cocos_point_to_egui(point) {
        push_event(Event::PointerMoved(position));
    }
}

pub fn push_cocos_pointer_button(
    point: geode_rs::cocos::CCPoint,
    button: PointerButton,
    pressed: bool,
) {
    if let Some(position) = cocos_point_to_egui(point) {
        push_event(Event::PointerButton {
            pos: position,
            button,
            pressed,
            modifiers: modifiers(),
        });
    }
}

pub fn push_pointer_gone() {
    push_event(Event::PointerGone);
}

pub fn wants_pointer_input() -> bool {
    let input = lock_input_state();
    input.visible && input.wants_pointer_input
}

pub fn wants_keyboard_input() -> bool {
    let input = lock_input_state();
    input.visible && input.wants_keyboard_input
}

pub fn mark_context_lost() {
    with_backend(|backend| backend.destroy_painter());
}

pub fn shutdown() {
    with_input_state(|input| {
        input.pending_events.clear();
        input.modifiers = Modifiers::default();
        input.visible = true;
        input.input_mode = InputMode::Default;
        input.wants_pointer_input = false;
        input.wants_keyboard_input = false;
    });
    with_backend(|backend| {
        backend.destroy_painter();
        backend.time_seconds = 0.0;
        backend.last_error = None;
        backend.ctx = egui::Context::default();
    });
    let mut last = LAST_FRAME_INFO
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *last = None;
}

pub fn try_paint_frame() -> Result<(), RenderError> {
    with_backend(|backend| {
        let result = backend.try_paint_frame();
        update_last_error(backend, &result);
        result
    })
}

pub fn paint_frame() {
    with_backend(|backend| {
        let result = backend.try_paint_frame();
        update_last_error(backend, &result);
    });
}

#[doc(hidden)]
pub unsafe fn dispatch_insert_text_hook(
    this: *mut geode_rs::cocos::CCIMEDispatcher,
    text: *const c_char,
    len: i32,
    key: geode_rs::cocos::enumKeyCodes,
) {
    if !is_visible_internal() {
        geode_rs::cocos::CCIMEDispatcher_dispatchInsertText(this, text, len, key);
        return;
    }

    if !text.is_null() && len > 0 {
        let bytes = slice::from_raw_parts(text.cast::<u8>(), len as usize);
        let text = String::from_utf8_lossy(bytes);
        if !text.is_empty() {
            push_text(text.into_owned());
        }
    }

    if !should_capture_keyboard() {
        geode_rs::cocos::CCIMEDispatcher_dispatchInsertText(this, text, len, key);
    }
}

#[doc(hidden)]
pub unsafe fn dispatch_delete_backward_hook(this: *mut geode_rs::cocos::CCIMEDispatcher) {
    if !is_visible_internal() {
        geode_rs::cocos::CCIMEDispatcher_dispatchDeleteBackward(this);
        return;
    }

    #[cfg(target_os = "ios")]
    {
        push_key(Key::Backspace, true, false);
        push_key(Key::Backspace, false, false);
    }

    if !should_capture_keyboard() {
        geode_rs::cocos::CCIMEDispatcher_dispatchDeleteBackward(this);
    }
}

#[doc(hidden)]
pub unsafe fn dispatch_keyboard_msg_hook(
    this: *mut geode_rs::cocos::CCKeyboardDispatcher,
    key: geode_rs::cocos::enumKeyCodes,
    is_key_down: bool,
    is_key_repeat: bool,
    time: f64,
) -> bool {
    let _ = this;
    update_modifiers_from_key_event(key, is_key_down);

    if !is_visible_internal() {
        return geode_rs::cocos::CCKeyboardDispatcher_dispatchKeyboardMSG(
            this,
            key,
            is_key_down,
            is_key_repeat,
            time,
        );
    }

    let should_eat_input = should_capture_keyboard();
    if should_eat_input || !is_key_down {
        if let Some(key) = cocos_key_to_egui_key(key) {
            push_key(key, is_key_down, is_key_repeat);
        }
    }

    if should_eat_input {
        false
    } else {
        geode_rs::cocos::CCKeyboardDispatcher_dispatchKeyboardMSG(
            this,
            key,
            is_key_down,
            is_key_repeat,
            time,
        )
    }
}

#[doc(hidden)]
pub unsafe fn dispatch_scroll_msg_hook(
    this: *mut geode_rs::cocos::CCMouseDispatcher,
    y: f32,
    x: f32,
) -> bool {
    if !is_visible_internal() {
        return geode_rs::cocos::CCMouseDispatcher_dispatchScrollMSG(this, y, x);
    }

    push_scroll(Vec2::new(x * SCROLL_MULTIPLIER, -y * SCROLL_MULTIPLIER));

    if should_capture_pointer() {
        true
    } else {
        geode_rs::cocos::CCMouseDispatcher_dispatchScrollMSG(this, y, x)
    }
}

#[doc(hidden)]
pub unsafe fn touch_dispatch_hook(
    this: *mut geode_rs::cocos::CCTouchDispatcher,
    touches: *mut geode_rs::cocos::CCSet,
    event: *mut geode_rs::cocos::CCEvent,
    ty: u32,
) {
    if !is_visible_internal() || touches.is_null() {
        geode_rs::cocos::CCTouchDispatcher_touches(this, touches, event, ty);
        return;
    }

    let touch = geode_rs::cocos::CCSet_anyObject(touches).cast::<geode_rs::cocos::CCTouch>();
    if touch.is_null() {
        geode_rs::cocos::CCTouchDispatcher_touches(this, touches, event, ty);
        return;
    }

    let point = geode_rs::cocos::CCTouch_getLocation(touch);
    if geode_rs::geode_mouse_position().x == 0.0 && geode_rs::geode_mouse_position().y == 0.0 {
        push_cocos_pointer_moved(geode_rs::cocos::CCPoint { x: point.x, y: point.y });
    }

    if should_capture_pointer() {
        match ty {
            TOUCH_BEGAN => push_cocos_pointer_button(point, PointerButton::Primary, true),
            TOUCH_ENDED | TOUCH_CANCELLED => {
                push_cocos_pointer_button(point, PointerButton::Primary, false)
            }
            _ => {}
        }
        if ty == TOUCH_MOVED {
            geode_rs::cocos::CCTouchDispatcher_touches(this, touches, event, TOUCH_CANCELLED);
        }
    } else {
        if ty != TOUCH_MOVED {
            push_cocos_pointer_button(point, PointerButton::Primary, false);
        }
        geode_rs::cocos::CCTouchDispatcher_touches(this, touches, event, ty);
    }
}

#[macro_export]
macro_rules! install_render_hooks {
    () => {
        #[cfg(any(target_os = "windows", target_os = "ios"))]
        mod __geode_egui_swap_buffers_hook {
            use geode_egui::geode_rs::classes::CCEGLView;

            #[derive(Default)]
            #[geode_egui::geode_rs::modify(CCEGLView)]
            struct GeodeEguiSwapBuffersHook;

            #[geode_egui::geode_rs::modify(CCEGLView)]
            impl GeodeEguiSwapBuffersHook {
                fn swap_buffers(&mut self, this: &mut CCEGLView) {
                    geode_egui::paint_frame();
                    this.swap_buffers();
                }
            }
        }

        #[cfg(not(any(target_os = "windows", target_os = "ios")))]
        mod __geode_egui_draw_scene_hook {
            use geode_egui::geode_rs::classes::CCDirector;

            #[derive(Default)]
            #[geode_egui::geode_rs::modify(CCDirector)]
            struct GeodeEguiDrawSceneHook;

            #[geode_egui::geode_rs::modify(CCDirector)]
            impl GeodeEguiDrawSceneHook {
                fn draw_scene(&mut self, this: &mut CCDirector) {
                    this.draw_scene();
                    geode_egui::paint_frame();
                }
            }
        }
    };
}

#[macro_export]
macro_rules! install_input_hooks {
    () => {
        mod __geode_egui_input_hooks {
            use geode_egui::geode_rs::classes::{CCIMEDispatcher, CCKeyboardDispatcher, CCTouchDispatcher};
            use geode_egui::geode_rs::cocos::{CCEvent, CCSet, enumKeyCodes};

            #[derive(Default)]
            #[geode_egui::geode_rs::modify(CCIMEDispatcher)]
            struct GeodeEguiImeHook;

            #[geode_egui::geode_rs::modify(CCIMEDispatcher)]
            impl GeodeEguiImeHook {
                fn dispatch_insert_text(
                    &mut self,
                    this: &mut CCIMEDispatcher,
                    text: *const std::ffi::c_char,
                    len: i32,
                    key: enumKeyCodes,
                ) {
                    unsafe {
                        geode_egui::dispatch_insert_text_hook(
                            this as *mut _ as *mut geode_egui::geode_rs::cocos::CCIMEDispatcher,
                            text,
                            len,
                            key,
                        );
                    }
                }

                fn dispatch_delete_backward(&mut self, this: &mut CCIMEDispatcher) {
                    unsafe {
                        geode_egui::dispatch_delete_backward_hook(
                            this as *mut _ as *mut geode_egui::geode_rs::cocos::CCIMEDispatcher,
                        );
                    }
                }
            }

            #[cfg(not(target_os = "ios"))]
            #[derive(Default)]
            #[geode_egui::geode_rs::modify(CCKeyboardDispatcher)]
            struct GeodeEguiKeyboardHook;

            #[cfg(not(target_os = "ios"))]
            #[geode_egui::geode_rs::modify(CCKeyboardDispatcher)]
            impl GeodeEguiKeyboardHook {
                fn dispatch_keyboard_msg(
                    &mut self,
                    this: &mut CCKeyboardDispatcher,
                    key: enumKeyCodes,
                    is_key_down: bool,
                    is_key_repeat: bool,
                    time: f64,
                ) -> bool {
                    unsafe {
                        geode_egui::dispatch_keyboard_msg_hook(
                            this as *mut _ as *mut geode_egui::geode_rs::cocos::CCKeyboardDispatcher,
                            key,
                            is_key_down,
                            is_key_repeat,
                            time,
                        )
                    }
                }
            }

            #[cfg(not(target_os = "ios"))]
            use geode_egui::geode_rs::classes::CCMouseDispatcher;

            #[cfg(not(target_os = "ios"))]
            #[derive(Default)]
            #[geode_egui::geode_rs::modify(CCMouseDispatcher)]
            struct GeodeEguiMouseHook;

            #[cfg(not(target_os = "ios"))]
            #[geode_egui::geode_rs::modify(CCMouseDispatcher)]
            impl GeodeEguiMouseHook {
                fn dispatch_scroll_msg(
                    &mut self,
                    this: &mut CCMouseDispatcher,
                    y: f32,
                    x: f32,
                ) -> bool {
                    unsafe {
                        geode_egui::dispatch_scroll_msg_hook(
                            this as *mut _ as *mut geode_egui::geode_rs::cocos::CCMouseDispatcher,
                            y,
                            x,
                        )
                    }
                }
            }

            #[derive(Default)]
            #[geode_egui::geode_rs::modify(CCTouchDispatcher)]
            struct GeodeEguiTouchHook;

            #[geode_egui::geode_rs::modify(CCTouchDispatcher)]
            impl GeodeEguiTouchHook {
                fn touches(
                    &mut self,
                    this: &mut CCTouchDispatcher,
                    touches: *mut CCSet,
                    event: *mut CCEvent,
                    ty: u32,
                ) {
                    unsafe {
                        geode_egui::touch_dispatch_hook(
                            this as *mut _ as *mut geode_egui::geode_rs::cocos::CCTouchDispatcher,
                            touches as *mut geode_egui::geode_rs::cocos::CCSet,
                            event as *mut geode_egui::geode_rs::cocos::CCEvent,
                            ty,
                        );
                    }
                }
            }
        }
    };
}

#[macro_export]
macro_rules! install_hooks {
    () => {
        geode_egui::install_render_hooks!();
        geode_egui::install_input_hooks!();
    };
}






















