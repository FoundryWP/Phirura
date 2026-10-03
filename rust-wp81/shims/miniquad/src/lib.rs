//! A `miniquad` shim for Windows Phone 8.1.
//!
//! `prpr` uses `miniquad` directly in a handful of places, mostly for render targets and the raw
//! GL calls in `core/render.rs`. This crate supplies the types so the code compiles, and forwards
//! what can be forwarded.
//!
//! # What cannot be honoured
//!
//! `gl` below is a stub, and deliberately so. WP8.1 has no OpenGL - no `opengl32.dll`, no EGL, no
//! ANGLE - and D3D11 has no equivalent of a multisampled renderbuffer attached to a framebuffer,
//! which is exactly what `core/render.rs` uses it for. Those calls are provided so the module
//! compiles; **`core/render.rs` is the one file Path B must rewrite against Win2D rather than
//! route through here.**
//!
//! Everything else (textures, render passes, uniform types) is an opaque handle: the C# side owns
//! the real resources and looks them up by id.

#![allow(non_upper_case_globals, non_camel_case_types, unused_variables, dead_code)]

pub use macroquad_compat_types::*;

/// Shared handle types.
///
/// Kept in a module so `macroquad`'s shim can re-export the same definitions instead of declaring
/// its own - two structs with the same shape but different types would not interoperate, and
/// `prpr` converts between them (`Texture2D::from_miniquad_texture`).
pub mod macroquad_compat_types {
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
    pub enum FilterMode {
        #[default]
        Linear,
        Nearest,
    }

    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Texture {
        pub id: u32,
        /// Dimensions are carried here rather than only on macroquad's `Texture2D` because
        /// `prpr::core::render` reads them off the miniquad handle when sizing render targets.
        pub width: u32,
        pub height: u32,
    }

    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
    pub struct RenderPass {
        pub id: u32,
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum TextureFormat {
        RGB8,
        RGBA8,
        Depth,
        Alpha,
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum TextureWrap {
        Repeat,
        Clamp,
        Mirror,
    }

    /// miniquad's own parameters - five fields, and `filter` rather than separate min/mag.
    ///
    /// macroquad has a *different* `TextureParams` with `min_filter`/`mag_filter`, and conflating
    /// the two is what made `prpr::core::line`'s literal fail: it writes the five-field form.
    #[derive(Clone, Debug)]
    pub struct TextureParams {
        pub format: TextureFormat,
        pub wrap: TextureWrap,
        pub filter: FilterMode,
        pub width: u32,
        pub height: u32,
    }

    impl Default for TextureParams {
        fn default() -> Self {
            TextureParams {
                format: TextureFormat::RGBA8,
                wrap: TextureWrap::Clamp,
                filter: FilterMode::Linear,
                width: 0,
                height: 0,
            }
        }
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum UniformType {
        Float1,
        Float2,
        Float3,
        Float4,
        Int1,
        Mat4,
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum TouchPhase {
        Started,
        Stationary,
        Moved,
        Ended,
        Cancelled,
    }

    /// `Eq` is dropped because the clear colour is a float tuple, matching upstream.
    #[derive(Copy, Clone, Debug, Default, PartialEq)]
    pub enum PassAction {
        #[default]
        Nothing,
        Clear {
            color: Option<(f32, f32, f32, f32)>,
        },
    }

    impl PassAction {
        /// The constructor form `prpr` uses.
        pub fn clear_color(color: (f32, f32, f32, f32)) -> Self {
            PassAction::Clear { color: Some(color) }
        }
    }

    /// Keyboard codes, for `EventHandler::key_down` / `key_up`.
    ///
    /// This crate's own `KeyCode`, distinct from the one `macroquad` re-exports - upstream has both
    /// too, and `prpr` only ever overrides `touch_event`, so nothing depends on them being the same
    /// type. The set is the keys `prpr` can actually see on a phone, which is none of them; it
    /// exists so the trait signature compiles.
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum KeyCode {
        Space,
        Enter,
        Escape,
        Backspace,
        Tab,
        Left,
        Right,
        Up,
        Down,
        LeftShift,
        RightShift,
        LeftControl,
        RightControl,
        A,
        C,
        D,
        E,
        F,
        Q,
        R,
        S,
        V,
        W,
        X,
        Z,
        Key0,
        Key1,
        Key2,
        Key3,
        Key4,
        Key5,
        Key6,
        Key7,
        Key8,
        Key9,
    }

    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
    pub struct KeyMods {
        pub shift: bool,
        pub ctrl: bool,
        pub alt: bool,
        pub logo: bool,
    }

    /// The context `prpr` passes around. The shell owns the real one; nothing here dereferences it.
    pub struct Context;

    pub mod conf {
        #[derive(Clone, Debug)]
        pub struct Conf {
            pub window_title: String,
            pub window_width: i32,
            pub window_height: i32,
            pub high_dpi: bool,
            pub fullscreen: bool,
            pub sample_count: i32,
            pub window_resizable: bool,
        }
        impl Default for Conf {
            fn default() -> Self {
                Conf {
                    window_title: String::new(),
                    window_width: 800,
                    window_height: 600,
                    high_dpi: false,
                    fullscreen: false,
                    sample_count: 1,
                    window_resizable: false,
                }
            }
        }
    }
}

pub mod conf {
    pub use super::macroquad_compat_types::conf::*;
}

impl Texture {
    pub fn new_render_texture(_ctx: &mut Context, params: TextureParams) -> Self {
        Texture {
            id: params.width ^ (params.height << 16) ^ params.format as u32,
            width: params.width,
            height: params.height,
        }
    }

    /// Rebuild a handle from an id handed out earlier.
    ///
    /// `prpr` keeps raw texture ids in its draw batches and reconstructs handles from them
    /// (`core/resource.rs`), so this has to round-trip. The format is accepted and ignored: on this
    /// platform the C# side owns the real texture and knows its format.
    pub fn from_raw_id(id: u32, _format: TextureFormat) -> Self {
        // Dimensions are unknown here - upstream does not carry them either. Callers that need
        // them reconstruct through `Texture2D`, which keeps its own copy.
        Texture { id, width: 0, height: 0 }
    }

    pub fn delete(&self) {}

    /// The GL object name, which `prpr::core::note` uses as a batch key so that consecutive notes
    /// sharing a texture merge into one draw call. Here the id is the shim's own handle, which
    /// serves the same purpose - grouping.
    pub fn gl_internal_id(&self) -> u32 {
        self.id
    }

    /// `prpr::core::resource` sets this when loading note textures. The shell decides its own
    /// sampling; the call is accepted so the loader's flow is unchanged.
    pub fn set_wrap(&self, _ctx: &mut Context, _wrap: TextureWrap) {}

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn set_filter(&self, _ctx: &mut Context, _filter: FilterMode) {}

    pub fn from_rgba8(ctx: &mut Context, width: u16, height: u16, bytes: &[u8]) -> Self {
        let _ = (ctx, bytes);
        Texture {
            id: 0,
            width: width as u32,
            height: height as u32,
        }
    }
}

impl RenderPass {
    pub fn new(_ctx: &mut Context, tex: Texture, _depth: Option<Texture>) -> Self {
        RenderPass { id: tex.id }
    }
    pub fn from_raw(_ctx: &mut Context, fbo: u32, _tex: Texture) -> Self {
        RenderPass { id: fbo }
    }
    /// `prpr` uses this only to key its draw batches. Batching belongs to the renderer here, so the
    /// handle is returned as-is rather than pretending to be a GL object name.
    pub fn gl_internal_id(&self, _ctx: &mut Context) -> u32 {
        self.id
    }

    /// The colour attachment. `prpr::core::render` reads it back to bind the result as a texture.
    pub fn texture(&self, _ctx: &mut Context) -> Texture {
        Texture { id: self.id, width: 0, height: 0 }
    }
}

/// Blend and pipeline state.
///
/// `prpr::ext` builds these to describe how sprites composite. The values are recorded and ignored:
/// the C# renderer decides its own blend state, and the ones prpr asks for (premultiplied alpha) are
/// what Win2D does by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Equation {
    Add,
    Subtract,
    ReverseSubtract,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendValue {
    SourceColor,
    SourceAlpha,
    DestinationColor,
    DestinationAlpha,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendFactor {
    Zero,
    One,
    Value(BlendValue),
    OneMinusValue(BlendValue),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlendState {
    pub equation: Equation,
    pub src_factor: BlendFactor,
    pub dst_factor: BlendFactor,
}

impl BlendState {
    pub fn new(equation: Equation, src_factor: BlendFactor, dst_factor: BlendFactor) -> Self {
        BlendState { equation, src_factor, dst_factor }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareFunc {
    Never,
    Less,
    Equal,
    LessOrEqual,
    Greater,
    NotEqual,
    GreaterOrEqual,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PrimitiveType {
    #[default]
    Triangles,
    Lines,
    LineStrip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StencilOp {
    Keep,
    Zero,
    Replace,
    IncrementAndClamp,
    DecrementAndClamp,
    Invert,
    IncrementAndWrap,
    DecrementAndWrap,
}

/// The per-face stencil state.
///
/// In the miniquad revision prpr targets, the test parameters live on the *face*, not on
/// `StencilState` - `prpr::ext::make_pipeline` assigns `test_func`, `test_ref`, `test_mask` and
/// `write_mask` inside a `StencilFaceState` literal. Putting them on the parent instead is the
/// obvious-looking layout and does not compile against this code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StencilFaceState {
    pub fail_op: StencilOp,
    pub depth_fail_op: StencilOp,
    pub pass_op: StencilOp,
    pub test_func: CompareFunc,
    pub test_ref: i32,
    pub test_mask: u32,
    pub write_mask: u32,
}

impl StencilFaceState {
    pub fn new(fail_op: StencilOp, depth_fail_op: StencilOp, pass_op: StencilOp) -> Self {
        StencilFaceState {
            fail_op,
            depth_fail_op,
            pass_op,
            test_func: CompareFunc::Always,
            test_ref: 0,
            test_mask: u32::MAX,
            write_mask: u32::MAX,
        }
    }
}

impl Default for StencilFaceState {
    fn default() -> Self {
        StencilFaceState::new(StencilOp::Keep, StencilOp::Keep, StencilOp::Keep)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StencilState {
    pub front: StencilFaceState,
    pub back: StencilFaceState,
}

impl StencilState {
    pub fn new(front: StencilFaceState, back: StencilFaceState) -> Self {
        StencilState { front, back }
    }
}

/// Mouse buttons. `prpr::judge` maps touches onto the left button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Unknown,
}

/// The event loop interface.
///
/// Every method has a default, so an implementation only overrides what it needs. On this platform
/// the shell owns the loop and calls these directly - `macroquad::utils::repeat_all_miniquad_input`
/// is a no-op here - which is why the trait can be this thin.
pub trait EventHandler {
    fn update(&mut self, _ctx: &mut Context) {}
    fn draw(&mut self, _ctx: &mut Context) {}
    fn resize(&mut self, _ctx: &mut Context, _width: i32, _height: i32) {}
    // The `_event` suffixes are miniquad's, not a style choice: `prpr::judge` implements these
    // exact names, and renaming them turns five working overrides into five "not a member of trait"
    // errors.
    fn key_down_event(&mut self, _ctx: &mut Context, _key: KeyCode, _mods: KeyMods, _repeat: bool) {}
    fn key_up_event(&mut self, _ctx: &mut Context, _key: KeyCode, _mods: KeyMods) {}
    fn char_event(&mut self, _ctx: &mut Context, _character: char, _mods: KeyMods, _repeat: bool) {}
    fn mouse_motion_event(&mut self, _ctx: &mut Context, _x: f32, _y: f32) {}
    fn mouse_wheel_event(&mut self, _ctx: &mut Context, _dx: f32, _dy: f32) {}
    fn mouse_button_down_event(&mut self, _ctx: &mut Context, _button: MouseButton, _x: f32, _y: f32) {}
    fn mouse_button_up_event(&mut self, _ctx: &mut Context, _button: MouseButton, _x: f32, _y: f32) {}
    fn touch_event(&mut self, _ctx: &mut Context, _phase: TouchPhase, _id: u64, _x: f32, _y: f32, _time: f64) {}
}

/// The rendering context.
///
/// Every method is a no-op: the C# renderer owns the real device. `prpr::particle` drives this
/// sequence directly, so the methods have to exist and accept the same arguments, but what they do
/// is recorded elsewhere - the draw commands come through `macroquad::QuadGl`.
impl Context {
    pub fn begin_default_pass(&mut self, _action: PassAction) {}
    pub fn begin_pass(&mut self, _pass: RenderPass, _action: PassAction) {}
    pub fn end_render_pass(&mut self) {}
    pub fn apply_viewport(&mut self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    pub fn apply_scissor_rect(&mut self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    pub fn apply_pipeline(&mut self, _pipeline: &Pipeline) {}
    pub fn apply_bindings(&mut self, _bindings: &Bindings) {}
    pub fn draw(&mut self, _base_element: i32, _num_elements: i32, _num_instances: i32) {}
    pub fn clear(&mut self, _color: Option<(f32, f32, f32, f32)>) {}
    pub fn viewport(&self) -> (i32, i32, i32, i32) {
        (0, 0, 0, 0)
    }
    pub fn render_pass(&self) -> Option<RenderPass> {
        None
    }
    pub fn texture(&self) -> Option<Texture> {
        None
    }
    pub fn get_active_render_pass(&self) -> RenderPass {
        RenderPass { id: 0 }
    }
}

impl Default for Context {
    fn default() -> Self {
        Context
    }
}

/// Shader uniform description, as `prpr::particle` builds it.
#[derive(Clone, Debug)]
pub struct UniformDesc {
    pub name: String,
    pub uniform_type: UniformType,
    pub array_count: i32,
}

impl UniformDesc {
    pub fn new(name: &str, uniform_type: UniformType) -> Self {
        UniformDesc {
            name: name.to_string(),
            uniform_type,
            array_count: 0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UniformBlockLayout {
    pub uniforms: Vec<UniformDesc>,
}

#[derive(Clone, Debug, Default)]
pub struct ShaderMeta {
    pub images: Vec<String>,
    pub uniforms: UniformBlockLayout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferType {
    VertexBuffer,
    IndexBuffer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VertexStep {
    #[default]
    PerVertex,
    PerInstance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertexFormat {
    Float1,
    Float2,
    Float3,
    Float4,
    Byte4,
    Short2,
    Short4,
    Mat4,
}

impl VertexFormat {
    /// Bytes per element. Used only to compute offsets in a layout.
    pub fn size(self) -> usize {
        match self {
            VertexFormat::Float1 => 4,
            VertexFormat::Float2 => 8,
            VertexFormat::Float3 => 12,
            VertexFormat::Float4 | VertexFormat::Byte4 | VertexFormat::Short2 => 16,
            VertexFormat::Short4 => 8,
            VertexFormat::Mat4 => 64,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VertexAttribute {
    pub name: String,
    pub format: VertexFormat,
    pub buffer_index: usize,
    pub offset: usize,
}

impl VertexAttribute {
    pub fn new(name: &str, format: VertexFormat) -> Self {
        VertexAttribute {
            name: name.to_string(),
            format,
            buffer_index: 0,
            offset: 0,
        }
    }

    /// Which vertex buffer the attribute reads from - `prpr::particle` interleaves per-vertex and
    /// per-instance data across two buffers.
    pub fn with_buffer(name: &str, format: VertexFormat, buffer_index: usize) -> Self {
        VertexAttribute {
            name: name.to_string(),
            format,
            buffer_index,
            offset: 0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct BufferLayout {
    pub stride: usize,
    pub attrs: Vec<VertexAttribute>,
    pub step_func: VertexStep,
}

impl BufferLayout {
    pub fn new(stride: usize, attrs: &[VertexAttribute]) -> Self {
        BufferLayout {
            stride,
            attrs: attrs.to_vec(),
            step_func: VertexStep::PerVertex,
        }
    }
    pub fn with_step_func(mut self, step_func: VertexStep) -> Self {
        self.step_func = step_func;
        self
    }
}

/// Opaque GPU resources.
///
/// On this platform the C# renderer owns every one of these; they exist so `prpr::particle` can
/// build its pipeline description and hand it over.
#[derive(Clone, Debug, Default)]
pub struct Shader {
    pub id: u32,
}

impl Shader {
    pub fn new(_ctx: &mut Context, _vertex: &str, _fragment: &str, meta: ShaderMeta) -> Result<Self, String> {
        Ok(Shader { id: meta.images.len() as u32 })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Pipeline {
    pub id: u32,
}

impl Pipeline {
    pub fn with_params(
        _ctx: &mut Context,
        _layout: BufferLayout,
        _meta: ShaderMeta,
        _params: PipelineParams,
    ) -> Self {
        Pipeline { id: 0 }
    }
}

/// `Copy` because macroquad's `Material` embeds one and is itself `Copy`, and `prpr` dereferences
/// a `&Material` into a value. `Default` because `MaterialParams` does `..Default::default()`.
#[derive(Copy, Clone, Debug)]
pub struct PipelineParams {
    pub color_blend: Option<BlendState>,
    pub alpha_blend: Option<BlendState>,
    pub stencil_test: Option<StencilState>,
    pub depth_test: Option<DepthState>,
    pub cull_face: CullFace,
    pub color_write: (bool, bool, bool, bool),
    pub depth_write: bool,
    pub primitive_type: PrimitiveType,
}

impl Default for PipelineParams {
    fn default() -> Self {
        PipelineParams {
            color_blend: None,
            alpha_blend: None,
            stencil_test: None,
            depth_test: None,
            cull_face: CullFace::Nothing,
            color_write: (true, true, true, true),
            depth_write: true,
            primitive_type: PrimitiveType::Triangles,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthState {
    pub write: bool,
    pub compare: CompareFunc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CullFace {
    #[default]
    Nothing,
    Front,
    Back,
}

#[derive(Clone, Debug, Default)]
pub struct Buffer {
    pub id: u32,
    pub len: usize,
}

impl Buffer {
    pub fn new<T>(_ctx: &mut Context, _buffer_type: BufferType, _usage: BufferUsage, data: &[T]) -> Self {
        Buffer { id: 0, len: core::mem::size_of_val(data) }
    }
    /// `prpr::particle` builds its vertex and index buffers with this.
    pub fn immutable<T>(_ctx: &mut Context, _buffer_type: BufferType, data: &[T]) -> Self {
        Buffer { id: 0, len: core::mem::size_of_val(data) }
    }
    pub fn stream<T>(_ctx: &mut Context, _buffer_type: BufferType, data: &[T]) -> Self {
        Buffer { id: 0, len: core::mem::size_of_val(data) }
    }

    pub fn update<T>(&self, _ctx: &mut Context, data: &[T]) {}
    pub fn size(&self) -> usize {
        self.len
    }
    pub fn delete(&self) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferUsage {
    Immutable,
    Dynamic,
}

#[derive(Clone, Debug, Default)]
pub struct Bindings {
    pub vertex_buffers: Vec<Buffer>,
    pub index_buffer: Buffer,
    pub images: Vec<Texture>,
}

/// The GL entry points `prpr::core::render` calls.
///
/// **These are stubs and cannot be made real.** They exist so `core/render.rs` compiles; that file
/// has to be rewritten for Win2D. Every function is a no-op and `glGetError` reports success, so
/// nothing here will silently produce wrong pixels - it will simply do nothing.
pub mod gl {
    pub type GLuint = u32;
    pub const GL_NO_ERROR: u32 = 0;
    pub const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
    pub const GL_NEAREST: u32 = 0x2600;
    pub const GL_READ_FRAMEBUFFER: u32 = 0x8CA8;
    pub const GL_DRAW_FRAMEBUFFER: u32 = 0x8CA9;
    pub const GL_FRAMEBUFFER: u32 = 0x8D40;
    pub const GL_RENDERBUFFER: u32 = 0x8D41;
    pub const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
    pub const GL_RGB8: u32 = 0x8051;

    /// `prpr::core::resource` passes this to `SafeTexture::with_filter`, and `prpr::ext` uses the
    /// rest when it builds GL pipeline state directly.
    pub type GLenum = u32;
    pub const GL_LINEAR: u32 = 0x2601;
    pub const GL_TEXTURE_2D: u32 = 0x0DE1;
    pub const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
    pub const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
    pub const GL_MAX_TEXTURE_SIZE: u32 = 0x0D33;
    pub const GL_RGBA: u32 = 0x1908;
    pub const GL_UNSIGNED_BYTE: u32 = 0x1401;
    pub const GL_LINEAR_MIPMAP_LINEAR: u32 = 0x2703;

    pub unsafe fn glTexParameteri(_target: u32, _pname: u32, _param: i32) {}

    pub unsafe fn glBindTexture(_target: u32, _texture: u32) {}
    pub unsafe fn glTexSubImage2D(
        _target: u32, _level: i32, _x: i32, _y: i32, _w: i32, _h: i32,
        _format: u32, _type_: u32, _pixels: *const u8,
    ) {
    }
    pub unsafe fn glGetIntegerv(_pname: u32, _params: *mut i32) {}
    pub unsafe fn glGenerateMipmap(_target: u32) {}
    pub unsafe fn glDeleteRenderbuffers(_n: i32, _ids: *const u32) {}
    pub unsafe fn glDeleteFramebuffers(_n: i32, _ids: *const u32) {}

    pub unsafe fn glBindFramebuffer(_target: u32, _fb: u32) {}
    pub unsafe fn glBlitFramebuffer(
        _sx0: i32, _sy0: i32, _sx1: i32, _sy1: i32,
        _dx0: i32, _dy0: i32, _dx1: i32, _dy1: i32,
        _mask: u32, _filter: u32,
    ) {
    }
    pub unsafe fn glGetError() -> u32 {
        GL_NO_ERROR
    }
    pub unsafe fn glGenRenderbuffers(_n: i32, _out: *mut u32) {}
    pub unsafe fn glBindRenderbuffer(_target: u32, _rb: u32) {}
    pub unsafe fn glRenderbufferStorageMultisample(
        _target: u32, _samples: i32, _fmt: u32, _w: i32, _h: i32,
    ) {
    }
    pub unsafe fn glGenFramebuffers(_n: i32, _out: *mut u32) {}
    pub unsafe fn glFramebufferRenderbuffer(_target: u32, _att: u32, _rbt: u32, _rb: u32) {}
}

/// Platform-specific hooks.
///
/// `set_interceptor_state` and `attach_jni_env` are Android-only in upstream miniquad and are
/// reached behind `cfg(target_os = "android")`, which is never true here - they are present so the
/// names resolve if that cfg ever evaluates true by accident.
pub mod native {
    pub fn set_interceptor_state(_s: bool) {}
    pub fn call_request_callback() {}
    pub unsafe fn attach_jni_env() {}

    pub mod windows {
        /// The desktop backend reads the OS uptime here. The shell owns the clock on this
        /// platform, so this reads the same value `macroquad`'s shim exposes.
        pub fn get_uptime() -> f64 {
            super::super::uptime_secs()
        }
    }
}

/// Filled in by the shell. `miniquad::native::windows::get_uptime` is the only consumer.
static mut UPTIME_SECS: f64 = 0.0;

pub fn set_uptime_secs(v: f64) {
    unsafe { UPTIME_SECS = v }
}

pub fn uptime_secs() -> f64 {
    unsafe { UPTIME_SECS }
}

/// `miniquad`'s entry point. The shell drives the loop, so this is never called; it exists so
/// `prpr::bin` compiles.
pub fn start<F>(_conf: conf::Conf, _f: F)
where
    F: 'static + FnMut(&mut Context),
{
}

pub fn start_async<F>(_conf: conf::Conf, _f: F)
where
    F: 'static + FnMut(&mut Context),
{
}
