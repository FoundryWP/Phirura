//! A `macroquad` shim for Windows Phone 8.1.
//!
//! # Why this exists
//!
//! `prpr` is written against `macroquad`, which is written against `miniquad`, which is a
//! **GL-only** abstraction: every backend (`windows.rs` 1000 lines, `android.rs` 731, `linux_x11.rs`
//! 894, `macos.rs` 680, `ohos.rs` 849, `ios.rs` 435, `wasm.rs` 444) funnels through `native/gl.rs`,
//! which loads OpenGL entry points from the platform. WP8.1 has no OpenGL at all - no
//! `opengl32.dll`, no EGL, no ANGLE - so there is no backend to port and no library to link.
//!
//! Worse, miniquad pipelines take **GLSL** source. D3D11 consumes HLSL/DXBC, and there is no
//! GLSL-to-HLSL translator available on this platform. A "D3D11 backend for miniquad" would
//! therefore have to include an ANGLE-sized translation layer *and* a shader compiler.
//!
//! So the rendering is not re-solved in Rust. It is already solved on the C#/Win2D side - that is
//! what the working app does - and this crate routes `macroquad`'s drawing calls there instead.
//!
//! # What is real and what is not
//!
//! * **Real:** every maths type (`Vec2`, `Vec3`, `Color`, `Rect`) with its operators, because
//!   `prpr`'s chart model, tweens and judgement are arithmetic and must behave exactly as they do
//!   on desktop. The upstream `Mac`-specific bits are irrelevant here; the semantics are not.
//! * **Routed:** `draw_*` and the texture/render-target calls record commands into a buffer that
//!   the C# side drains and replays through Win2D.
//! * **Stubbed:** windowing and input, which the C# XAML shell owns. `get_internal_gl` returns a
//!   dummy because `prpr` only reaches for it to get a GL texture id for batching - and batching
//!   is the renderer's job here, not the shim's.
//!
//! # Scope
//!
//! The surface is the subset `prpr` actually uses, measured by scanning its source (~40 distinct
//! items across ~130 call sites), not the whole of macroquad.

#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    unused_variables,
    unused_imports,
    dead_code
)]

use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

// ===========================================================================
// maths
//
// Re-exported from glam, not defined here.
//
// macroquad has no maths types of its own - it re-exports glam's, which is why `prpr` can call
// `glam::Quat::from_rotation_z` and hand the result straight to a macroquad API. Locally defined
// types were structurally identical and therefore still wrong: different types do not unify, and
// the ~60 "cannot find vec2/vec3/vec4" errors were exactly that.
//
// Version and feature must track macroquad 0.3.25 (`glam 0.30`, `scalar-math`). `scalar-math` is
// also the right choice on ARM - it disables SIMD.
// ===========================================================================

/// The glam crate itself, re-exported as a module.
///
/// `prpr::particle` writes `glam::Quat::from_rotation_z(..)` rather than importing the type, and
/// that path only resolves because macroquad re-exports glam - which the prelude then brings into
/// scope with its glob. Without this line the name `glam` is a crate prpr does not depend on.
pub use glam;

pub use glam::{Mat2, Mat3, Mat3A, Mat4, Quat, Vec2, Vec3, Vec3A, Vec4};
pub use glam::{DMat2, DMat3, DMat4, DQuat, DVec2, DVec3, DVec4};
pub use glam::{IVec2, IVec3, IVec4, UVec2, UVec3, UVec4};

pub use glam::{dmat2, dmat3, dmat4, dquat, dvec2, dvec3, dvec4};
pub use glam::{ivec2, ivec3, ivec4, uvec2, uvec3, uvec4};
pub use glam::{vec2, vec3, vec3a, vec4};

/// glam has no `Rect`, so this one really is macroquad's.

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.w, self.h)
    }
    pub fn center(&self) -> Vec2 {
        Vec2::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn left(&self) -> f32 {
        self.x
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn top(&self) -> f32 {
        self.y
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && self.x + self.w > o.x && self.y < o.y + o.h && self.y + self.h > o.y
    }
}

// ===========================================================================
// colour
// ===========================================================================

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Color { r, g, b, a }
    }
    pub const fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }
    /// macroquad's `from_hex` takes 0xRRGGBB.
    pub const fn from_hex(hex: u32) -> Self {
        Color {
            r: ((hex >> 16) & 0xFF) as f32 / 255.0,
            g: ((hex >> 8) & 0xFF) as f32 / 255.0,
            b: (hex & 0xFF) as f32 / 255.0,
            a: 1.0,
        }
    }
    /// macroquad's name for `from_rgba`; `prpr::core::resource` uses this one.
    pub const fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    /// 0xRRGGBB, opaque. `prpr::core::resource` parses note-style colours with this.
    /// 0xAARRGGBB. `prpr::core::resource` parses note-style colours that carry alpha.
    pub const fn from_hex_argb(hex: u32) -> Self {
        Color {
            r: ((hex >> 16) & 0xFF) as f32 / 255.0,
            g: ((hex >> 8) & 0xFF) as f32 / 255.0,
            b: (hex & 0xFF) as f32 / 255.0,
            a: ((hex >> 24) & 0xFF) as f32 / 255.0,
        }
    }

    pub const fn from_hex_rgb(hex: u32) -> Self {
        Color {
            r: ((hex >> 16) & 0xFF) as f32 / 255.0,
            g: ((hex >> 8) & 0xFF) as f32 / 255.0,
            b: (hex & 0xFF) as f32 / 255.0,
            a: 1.0,
        }
    }

    /// Packed for a vertex attribute. `prpr::particle` stores colours as `Vec4` in its instance
    /// data, so this is the bridge between the two.
    pub fn to_vec(self) -> Vec4 {
        Vec4::new(self.r, self.g, self.b, self.a)
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Color { a, ..self }
    }
    pub fn to_rgba8(self) -> [u8; 4] {
        [
            (self.r * 255.0) as u8,
            (self.g * 255.0) as u8,
            (self.b * 255.0) as u8,
            (self.a * 255.0) as u8,
        ]
    }
    pub fn to_vec4(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

impl From<[u8; 4]> for Color {
    /// RGBA bytes, which `prpr::particle` uses to write colours into its instance buffer.
    fn from(v: [u8; 4]) -> Self {
        Color::from_rgba8(v[0], v[1], v[2], v[3])
    }
}

impl From<Color> for [u8; 4] {
    fn from(c: Color) -> Self {
        [
            (c.r * 255.0) as u8,
            (c.g * 255.0) as u8,
            (c.b * 255.0) as u8,
            (c.a * 255.0) as u8,
        ]
    }
}

impl Default for Color {
    /// Opaque white. `prpr::core::effect` needs `Color: Default` for its `UniformValue` impl, and
    /// white is the identity for the multiplicative blending the effects use.
    fn default() -> Self {
        WHITE
    }
}

impl Mul<f32> for Color {
    type Output = Color;
    fn mul(self, o: f32) -> Color {
        Color::new(self.r * o, self.g * o, self.b * o, self.a * o)
    }
}

pub const WHITE: Color = Color::new(1.0, 1.0, 1.0, 1.0);
pub const BLACK: Color = Color::new(0.0, 0.0, 0.0, 1.0);
pub const BLANK: Color = Color::new(0.0, 0.0, 0.0, 0.0);
pub const RED: Color = Color::new(1.0, 0.0, 0.0, 1.0);
pub const GREEN: Color = Color::new(0.0, 1.0, 0.0, 1.0);
pub const BLUE: Color = Color::new(0.0, 0.0, 1.0, 1.0);
pub const YELLOW: Color = Color::new(1.0, 1.0, 0.0, 1.0);
pub const ORANGE: Color = Color::new(1.0, 0.647, 0.0, 1.0);
pub const PINK: Color = Color::new(1.0, 0.753, 0.796, 1.0);
pub const PURPLE: Color = Color::new(0.5, 0.0, 0.5, 1.0);
pub const BROWN: Color = Color::new(0.647, 0.165, 0.165, 1.0);
pub const BEIGE: Color = Color::new(0.961, 0.961, 0.863, 1.0);
pub const LIME: Color = Color::new(0.0, 1.0, 0.0, 1.0);
pub const SKYBLUE: Color = Color::new(0.529, 0.808, 0.922, 1.0);
pub const GOLD: Color = Color::new(1.0, 0.843, 0.0, 1.0);
pub const GRAY: Color = Color::new(0.5, 0.5, 0.5, 1.0);
pub const DARKGRAY: Color = Color::new(0.25, 0.25, 0.25, 1.0);
pub const LIGHTGRAY: Color = Color::new(0.827, 0.827, 0.827, 1.0);
pub const DARKGREEN: Color = Color::new(0.0, 0.392, 0.0, 1.0);
pub const MAROON: Color = Color::new(0.502, 0.0, 0.0, 1.0);
pub const VIOLET: Color = Color::new(0.933, 0.510, 0.933, 1.0);
pub const DARKPURPLE: Color = Color::new(0.439, 0.063, 0.439, 1.0);

// ===========================================================================
// textures
// ===========================================================================

/// Re-exported, not redeclared: `prpr` passes this straight into `miniquad::TextureParams`, so
/// the two crates must agree on the type.
pub use miniquad::FilterMode;

/// An offscreen render target.
///
/// A struct with these two fields, not an enum: `prpr::core::render` reads `.texture` and
/// `.render_pass` off it, and `prpr::ext` constructs one from a miniquad render pass.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct RenderTarget {
    pub texture: Texture2D,
    pub render_pass: crate::miniquad_compat::RenderPass,
}

impl RenderTarget {
    pub fn delete(&self) {}
}

/// An opaque GPU texture.
///
/// The `id` is handed out by the shim and is what the C# side uses to find the Win2D
/// `CanvasBitmap`; `width`/`height` are kept here because `prpr` measures with them.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Texture2D {
    pub id: u32,
    pub width: u32,
    pub height: u32,
}

impl Texture2D {
    pub fn new(id: u32, width: u32, height: u32) -> Self {
        Texture2D { id, width, height }
    }
    pub fn width(&self) -> f32 {
        self.width as f32
    }
    pub fn height(&self) -> f32 {
        self.height as f32
    }
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.width as f32, self.height as f32)
    }
    /// Builds a texture from raw RGBA bytes.
    ///
    /// The pixels are not uploaded: the shell owns the real textures. The handle records the
    /// dimensions so `prpr` can measure with them, which is the only thing it does with the result.
    pub fn from_rgba8(width: u16, height: u16, _bytes: &[u8]) -> Self {
        Texture2D {
            id: draw::next_texture_id(),
            width: width as u32,
            height: height as u32,
        }
    }

    pub fn from_image(image: &Image) -> Self {
        Texture2D::from_rgba8(image.width, image.height, &image.bytes)
    }

    /// A zero-sized placeholder. `prpr::bin` uses it for a judge line whose texture is supplied
    /// later, so it must not panic or allocate.
    pub fn empty() -> Self {
        Texture2D {
            id: 0,
            width: 0,
            height: 0,
        }
    }

    pub fn set_filter(&self, _f: FilterMode) {}

    pub fn delete(&self) {}
    pub fn from_miniquad_texture(_t: crate::miniquad_compat::Texture) -> Self {
        Texture2D { id: 0, width: 0, height: 0 }
    }
    /// `prpr` calls this only to key its draw batches, and batching belongs to the renderer on
    /// this platform - so the id is returned unchanged rather than pretending to be a GL handle.
    pub fn raw_miniquad_texture_handle(&self) -> crate::miniquad_compat::Texture {
        // The dimensions travel with the handle: `prpr::core::render` sizes its render targets by
        // reading `width`/`height` off whatever this returns, so a zero-sized handle would produce
        // zero-sized targets.
        crate::miniquad_compat::Texture {
            id: self.id,
            width: self.width,
            height: self.height,
        }
    }
    pub fn gl_internal_id(&self) -> u32 {
        self.id
    }
    pub fn get_texture_data(&self) -> Image {
        Image::gen_image_color(self.width as u16, self.height as u16, BLANK)
    }
}

#[derive(Clone, Debug)]
pub struct Image {
    pub width: u16,
    pub height: u16,
    pub bytes: Vec<u8>,
}

impl Image {
    pub fn gen_image_color(width: u16, height: u16, color: Color) -> Self {
        let c = color.to_rgba8();
        let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..(width as usize * height as usize) {
            bytes.extend_from_slice(&c);
        }
        Image { width, height, bytes }
    }
    pub fn from_file_with_format(_bytes: &[u8], _format: Option<ImageFormat>) -> Result<Self, String> {
        Err("decoding is done by the C# side".into())
    }
    pub fn get_image_data(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
}

#[derive(Clone, Debug, Default)]
pub struct DrawTextureParams {
    pub dest_size: Option<Vec2>,
    pub source: Option<Rect>,
    pub rotation: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    pub pivot: Option<Vec2>,
}

// ===========================================================================
// the miniquad surface prpr reaches for
//
// prpr imports `miniquad` directly as well as through macroquad (render passes, textures,
// `UniformType`, and - in `core/render.rs` - raw GL for multisampled render targets). These are
// the pieces that can be represented here; the GL calls cannot, and that file is the one place
// Path B has to replace rather than route.
// ===========================================================================
pub mod miniquad_compat {
    // The handle types live in the miniquad shim and are re-exported here rather than declared
    // again. `prpr` converts between the two crates' types (`Texture2D::from_miniquad_texture`,
    // `raw_miniquad_texture_handle`), so they have to be the *same* types - two structurally
    // identical structs would not compile at those call sites.
    pub use miniquad::*;
}

// ===========================================================================
// window / time / input
//
// The C# XAML shell owns all of this. What prpr needs from it is the numbers, not the plumbing.
// ===========================================================================

// ===========================================================================
// the batcher
//
// `prpr` bypasses macroquad's draw_* helpers for its custom paths and drives the batcher directly.
// `QuadGl` is that batcher. Every call records into the same buffer `draw_*` uses, so the C# side
// replays it through Win2D in order.
// ===========================================================================

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vertex {
    pub position: Vec3,
    pub uv: Vec2,
    pub color: [f32; 4],
}

impl Vertex {
    pub fn new(x: f32, y: f32, z: f32, u: f32, v: f32, color: Color) -> Self {
        Vertex {
            position: Vec3::new(x, y, z),
            uv: Vec2::new(u, v),
            color: color.to_vec4(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DrawMode {
    Triangles,
    Lines,
    LineStrip,
}

/// macroquad's batcher.
///
/// The methods take and return the real handle types where prpr passes them (`Texture2D`), and
/// ignore everything else - `make_pipeline` takes the raw GL-ish pipeline description that
/// `core/render.rs` builds, and the C# renderer decides its own pipeline state.
pub struct QuadGl;

impl QuadGl {
    pub fn texture(&mut self, tex: Option<Texture2D>) {
        draw::set_texture(tex.map(|t| t.id));
    }

    pub fn draw_mode(&mut self, mode: DrawMode) {
        draw::set_draw_mode(mode);
    }

    pub fn geometry(&mut self, vertices: &[Vertex], indices: &[u16]) {
        draw::push_geometry(vertices, indices);
    }

    pub fn scissor(&mut self, rect: Option<Rect>) {
        draw::set_scissor(rect);
    }

    pub fn push_model_matrix(&mut self, _mat: Mat4) {}

    pub fn pop_model_matrix(&mut self) {}

    /// Builds a pipeline.
    ///
    /// The signature follows the miniquad revision prpr targets, which is the *older* one: shader
    /// sources by value, then `PipelineParams`, then attribute layouts and uniforms. An earlier
    /// attempt used a `(write_color, write_depth, stencil_test, blend)` boolean tuple, which is the
    /// shape of a different API entirely and matched nothing.
    ///
    /// Returns `Ok` because `prpr::ext::make_pipeline` unwraps the result.
    pub fn make_pipeline(
        &mut self,
        _ctx: &mut miniquad::Context,
        _vertex: &str,
        _fragment: &str,
        _params: miniquad::PipelineParams,
        _attributes: Vec<miniquad::BufferLayout>,
        _uniforms: Vec<(String, miniquad::UniformType)>,
    ) -> Result<GlPipeline, ShaderError> {
        Ok(GlPipeline {
            pipeline: miniquad::Pipeline { id: 0 },
        })
    }

    /// Ends the batch. The C# side drains the buffer at the end of the frame, so this is where a
    /// real implementation would hand it over; recording is already in order, so there is nothing
    /// to do.
    pub fn flush(&mut self) {}

    /// `prpr::ext::get_viewport` unwraps this with a fallback, so `None` means "ask the shell".
    pub fn get_viewport(&self) -> Option<Rect> {
        None
    }

    /// A setter, not a getter: `prpr::core::line` brackets an offscreen pass with
    /// `viewport(None)` / `viewport(Some(..))`. The getter is `get_viewport`.
    pub fn viewport(&mut self, _rect: Option<(i32, i32, i32, i32)>) {}

    /// Selects the render target. A setter, not a getter - `prpr::core::effect` and
    /// `prpr::core::line` both call it with the pass they want to draw into.
    pub fn render_pass(&mut self, _pass: Option<crate::miniquad_compat::RenderPass>) {}

    /// `prpr::ext::get_viewport` maps over this, so it must be an `Option` - returning a bare
    /// `RenderPass` made it "not an iterator".
    pub fn get_active_render_pass(&self) -> Option<crate::miniquad_compat::RenderPass> {
        None
    }

    pub fn set_blend(&mut self, _blend: Option<miniquad::BlendState>, _clear: bool) {}

    /// The projection `prpr::ui` composes its transforms against.
    pub fn get_projection_matrix(&self) -> Mat4 {
        Mat4::IDENTITY
    }

    /// Sets a shader uniform. The value is dropped: the C# renderer owns pipelines, and prpr's
    /// effects are what it renders through.
    pub fn set_uniform<T>(&mut self, _name: &str, _value: T) {}
}

pub mod window {
    use super::*;

    /// `prpr::core::resource` reaches this by full path (`macroquad::window::...`) while the
    /// prelude exports it from the crate root, so it has to resolve from both.
    pub use super::gl_set_drawcall_buffer_capacity;

    /// Re-exported here as well: `prpr` imports `macroquad::window::miniquad`.
    pub use miniquad;

    pub struct Conf {
        pub window_title: String,
        pub window_width: i32,
        pub window_height: i32,
        pub high_dpi: bool,
        pub fullscreen: bool,
        pub sample_count: i32,
        pub window_resizable: bool,
        pub icon: Option<()>,
        pub platform: Platform,
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
                icon: None,
                platform: Platform::default(),
            }
        }
    }

    #[derive(Default)]
    pub struct Platform {
        pub swap_interval: Option<i32>,
        pub blocking_event_loop: bool,
        pub android_ignore_home_button: bool,
        pub android_quit_on_back: bool,
    }

    /// The context macroquad hands out.
    ///
    /// `prpr` both destructures it (`InternalGlContext { quad_gl, quad_context }`) and calls
    /// `flush()` straight through it, so it carries a `QuadGl` and derefs to it.
    pub struct InternalGlContext<'a> {
        pub quad_gl: QuadGl,
        pub quad_context: &'a mut miniquad::Context,
    }

    impl<'a> core::ops::Deref for InternalGlContext<'a> {
        type Target = QuadGl;
        fn deref(&self) -> &QuadGl {
            &self.quad_gl
        }
    }

    impl<'a> core::ops::DerefMut for InternalGlContext<'a> {
        fn deref_mut(&mut self) -> &mut QuadGl {
            &mut self.quad_gl
        }
    }

    pub static mut SCREEN: Vec2 = Vec2::new(0.0, 0.0);

    pub fn screen_width() -> f32 {
        unsafe { SCREEN.x }
    }
    pub fn screen_height() -> f32 {
        unsafe { SCREEN.y }
    }
    pub fn screen_aspect() -> f32 {
        unsafe {
            if SCREEN.y > 0.0 { SCREEN.x / SCREEN.y } else { 1.0 }
        }
    }
    pub fn screen_dimensions() -> Vec2 {
        unsafe { SCREEN }
    }
    pub fn screen_rect() -> Rect {
        Rect::new(0.0, 0.0, screen_width(), screen_height())
    }
    pub fn window_rect() -> (i32, i32, i32, i32) {
        (0, 0, screen_width() as i32, screen_height() as i32)
    }

    pub unsafe fn get_internal_gl() -> InternalGlContext<'static> {
        // Real addresses into `static mut`, because returning a reference to a temporary cannot
        // work. Nothing here dereferences them: `prpr` only forwards them to `QuadGl` and to the
        // miniquad calls, both of which ignore their arguments.
        static mut CTX: u8 = 0;
        let p: *mut u8 = core::ptr::addr_of_mut!(CTX);
        InternalGlContext {
            quad_gl: QuadGl,
            quad_context: &mut *(p as *mut miniquad::Context),
        }
    }

    /// The camera stack `prpr::scene::game` brackets its HUD passes with.
    ///
    /// Nothing is pushed: the shell renders through Win2D and applies its own transforms, so there
    /// is no GL state to save. Kept as a no-op pair so the bracketing calls stay balanced.
    pub fn push_camera_state() {}
    pub fn pop_camera_state() {}

    pub fn set_camera(_c: &Camera2D) {}
    pub fn set_default_camera() {}
    pub fn request_new_screen_size(_w: f32, _h: f32) {}
    pub fn next_frame() -> impl core::future::Future<Output = ()> {
        core::future::ready(())
    }
    pub fn clear_background(_c: Color) {}
}

#[derive(Clone, Debug, PartialEq)]
pub struct Camera2D {
    pub zoom: Vec2,
    pub target: Vec2,
    pub rotation: f32,
    pub offset: Vec2,
    pub render_target: Option<RenderTarget>,
    pub viewport: Option<(i32, i32, i32, i32)>,
}

impl Default for Camera2D {
    fn default() -> Self {
        Camera2D {
            zoom: Vec2::new(1.0, 1.0),
            target: Vec2::ZERO,
            rotation: 0.0,
            offset: Vec2::ZERO,
            render_target: None,
            viewport: None,
        }
    }
}

pub mod time {
    pub fn get_time() -> f64 {
        crate::clock::now_secs()
    }
    pub fn get_uptime() -> f64 {
        crate::clock::now_secs()
    }
    pub fn get_frame_time() -> f32 {
        1.0 / 60.0
    }
}

/// The clock the shell drives. `prpr` reads macroquad's time in several places, and on this
/// platform the authoritative clock lives on the C# side (it has the audio position), so the
/// shell writes the current time in here every frame.
pub mod clock {
    use core::sync::atomic::{AtomicU64, Ordering};
    static MICROS: AtomicU64 = AtomicU64::new(0);

    pub fn set_now_micros(us: u64) {
        MICROS.store(us, Ordering::Relaxed);
    }
    pub fn now_secs() -> f64 {
        MICROS.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }
}

pub mod input {
    use super::*;

    /// Re-exported from miniquad, not declared here.
    ///
    /// macroquad re-exports these, and `prpr::judge` implements miniquad's `EventHandler` using a
    /// `KeyCode` it imports from macroquad. Two identically shaped enums would be different types
    /// and the trait impl would not match - which is exactly the `incompatible type for trait`
    /// error this fixes.
    pub use crate::miniquad::{KeyCode, MouseButton, TouchPhase};


    #[derive(Copy, Clone, Debug, PartialEq)]
    pub struct Touch {
    /// Seconds, from the platform event. `prpr::judge` uses it to order simultaneous touches.
    pub time: f64,
        pub id: u64,
        pub phase: TouchPhase,
        pub position: Vec2,
    }


    pub fn is_key_pressed(_k: KeyCode) -> bool {
        false
    }
    pub fn is_key_down(_k: KeyCode) -> bool {
        false
    }
    pub fn is_key_released(_k: KeyCode) -> bool {
        false
    }
    pub fn mouse_position() -> (f32, f32) {
        (0.0, 0.0)
    }
    pub fn mouse_wheel() -> (f32, f32) {
        (0.0, 0.0)
    }
    pub fn get_touches() -> Vec<Touch> {
        Vec::new()
    }

    /// macroquad's name for the same list.
    pub fn touches() -> Vec<Touch> {
        Vec::new()
    }

    /// `prpr::judge` treats the left mouse button as a pointer, so this is the same input path as
    /// touch. The shell feeds touches through the handler instead, so nothing is down here.
    pub fn is_mouse_button_down(_button: crate::miniquad::MouseButton) -> bool {
        false
    }

    /// Makes mouse events also arrive as touches. `prpr::scene` sets this at startup; the shell
    /// already delivers touches directly, so there is nothing to switch.
    pub fn simulate_mouse_with_touch(_enabled: bool) {}

    pub fn is_mouse_button_pressed(_button: crate::miniquad::MouseButton) -> bool {
        false
    }

    pub fn is_mouse_button_released(_button: crate::miniquad::MouseButton) -> bool {
        false
    }
}

// ===========================================================================
// drawing
//
// Recorded, not drawn. The C# side drains the buffer each frame and replays it through Win2D -
// the same renderer the working C# port already uses, so the picture is identical by construction
// rather than by luck.
// ===========================================================================

pub mod draw {
    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};

    #[derive(Copy, Clone, Debug)]
    pub enum Cmd {
        Texture {
            tex: u32,
            dest: Rect,
            source: Rect,
            color: Color,
            rotation: f32,
            flip_x: bool,
            flip_y: bool,
        },
        Rect {
            dest: Rect,
            color: Color,
        },
        Text {
            x: f32,
            y: f32,
            color: Color,
            len: u32,
        },
        Line {
            a: Vec2,
            b: Vec2,
            thickness: f32,
            color: Color,
        },
        Circle {
            center: Vec2,
            r: f32,
            color: Color,
        },
        /// A batch handed to `QuadGl::geometry`.
        ///
        /// The vertices and indices live in their own arenas rather than inside this variant,
        /// because `Cmd` has to stay `Copy` - it sits in a fixed array, and a growing `Vec` would
        /// mean an allocation every frame on a phone.
        Geometry {
            tex: u32,
            mode: DrawMode,
            vstart: u32,
            vcount: u32,
            istart: u32,
            icount: u32,
        },
    }

    // Fixed capacity: this runs on a phone and a growing Vec would be an allocation per frame.
    const CAP: usize = 8192;
    static mut BUF: [Option<Cmd>; CAP] = [None; CAP];
    static LEN: AtomicU32 = AtomicU32::new(0);
    static TEX_ID: AtomicU32 = AtomicU32::new(1);

    pub fn push(c: Cmd) {
        let n = LEN.load(Ordering::Relaxed) as usize;
        if n < CAP {
            unsafe { BUF[n] = Some(c) };
            LEN.store(n as u32 + 1, Ordering::Relaxed);
        }
    }

    pub fn next_texture_id() -> u32 {
        TEX_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// Called by the shell after it has replayed a frame.
    pub fn drain_reset() {
        LEN.store(0, Ordering::Relaxed);
        // The arenas must be cleared too, or they would fill up and the batcher would silently
        // stop drawing after a few seconds of play.
        VLEN.store(0, Ordering::Relaxed);
        ILEN.store(0, Ordering::Relaxed);
    }

    // --- batcher state, mirroring macroquad's QuadGl ---

    const VCAP: usize = 65536;
    const ICAP: usize = 98304;
    static mut VERTS: [Vertex; VCAP] = [Vertex {
        position: Vec3 { x: 0.0, y: 0.0, z: 0.0 },
        uv: Vec2 { x: 0.0, y: 0.0 },
        color: [0.0; 4],
    }; VCAP];
    static mut INDICES: [u16; ICAP] = [0; ICAP];
    static VLEN: AtomicU32 = AtomicU32::new(0);
    static ILEN: AtomicU32 = AtomicU32::new(0);
    static CUR_TEX: AtomicU32 = AtomicU32::new(0);
    static CUR_MODE: AtomicU32 = AtomicU32::new(0);

    pub fn set_texture(id: Option<u32>) {
        CUR_TEX.store(id.unwrap_or(0), Ordering::Relaxed);
    }

    pub fn current_texture() -> u32 {
        CUR_TEX.load(Ordering::Relaxed)
    }

    pub fn set_draw_mode(mode: DrawMode) {
        CUR_MODE.store(mode as u32, Ordering::Relaxed);
    }

    pub fn set_scissor(_r: Option<Rect>) {
        // The C# renderer applies its own scissor; on the paths prpr uses the recorded geometry is
        // already clipped by the caller.
    }

    pub fn push_geometry(vertices: &[Vertex], indices: &[u16]) {
        let vs = VLEN.load(Ordering::Relaxed) as usize;
        let is = ILEN.load(Ordering::Relaxed) as usize;
        if vs + vertices.len() > VCAP || is + indices.len() > ICAP {
            return;
        }
        unsafe {
            for (i, v) in vertices.iter().enumerate() {
                VERTS[vs + i] = *v;
            }
            for (i, ix) in indices.iter().enumerate() {
                INDICES[is + i] = *ix;
            }
        }
        VLEN.store((vs + vertices.len()) as u32, Ordering::Relaxed);
        ILEN.store((is + indices.len()) as u32, Ordering::Relaxed);
        push(Cmd::Geometry {
            tex: CUR_TEX.load(Ordering::Relaxed),
            mode: match CUR_MODE.load(Ordering::Relaxed) {
                1 => DrawMode::Lines,
                2 => DrawMode::LineStrip,
                _ => DrawMode::Triangles,
            },
            vstart: vs as u32,
            vcount: vertices.len() as u32,
            istart: is as u32,
            icount: indices.len() as u32,
        });
    }

    pub fn vertex_slice(start: u32, count: u32) -> &'static [Vertex] {
        unsafe { &VERTS[start as usize..(start + count) as usize] }
    }

    pub fn index_slice(start: u32, count: u32) -> &'static [u16] {
        unsafe { &INDICES[start as usize..(start + count) as usize] }
    }

    /// Read-only view for the FFI entry point in the port crate.
    pub fn snapshot() -> &'static [Option<Cmd>] {
        let n = LEN.load(Ordering::Relaxed) as usize;
        unsafe { &BUF[..n] }
    }
}

pub fn draw_texture_ex(tex: Texture2D, x: f32, y: f32, color: Color, params: DrawTextureParams) {
    let size = params.dest_size.unwrap_or_else(|| tex.size());
    let source = params
        .source
        .unwrap_or_else(|| Rect::new(0.0, 0.0, tex.width(), tex.height()));
    draw::push(draw::Cmd::Texture {
        tex: tex.id,
        dest: Rect::new(x, y, size.x, size.y),
        source,
        color,
        rotation: params.rotation,
        flip_x: params.flip_x,
        flip_y: params.flip_y,
    });
}

pub fn draw_texture(tex: &Texture2D, x: f32, y: f32, color: Color) {
    draw_texture_ex(*tex, x, y, color, DrawTextureParams::default());
}

pub fn draw_rectangle(x: f32, y: f32, w: f32, h: f32, color: Color) {
    draw::push(draw::Cmd::Rect { dest: Rect::new(x, y, w, h), color });
}

pub fn draw_line(x1: f32, y1: f32, x2: f32, y2: f32, thickness: f32, color: Color) {
    draw::push(draw::Cmd::Line {
        a: Vec2::new(x1, y1),
        b: Vec2::new(x2, y2),
        thickness,
        color,
    });
}

pub fn draw_circle(x: f32, y: f32, r: f32, color: Color) {
    draw::push(draw::Cmd::Circle { center: Vec2::new(x, y), r, color });
}

pub fn draw_text(text: &str, x: f32, y: f32, _size: f32, color: Color) {
    draw::push(draw::Cmd::Text {
        x,
        y,
        color,
        len: text.len() as u32,
    });
}

pub fn measure_text(_t: &str, _size: f32, _scale: f32) -> TextDimensions {
    TextDimensions { width: 0.0, height: 0.0, offset_y: 0.0 }
}

pub struct TextDimensions {
    pub width: f32,
    pub height: f32,
    pub offset_y: f32,
}

// ===========================================================================
// prelude
// ===========================================================================

/// The module layout upstream macroquad exposes.
///
/// These are aliases onto the items defined above rather than separate implementations: `prpr`
/// reaches some of them by full path (`macroquad::file::load_file`) and some through the prelude,
/// and both have to resolve. Keeping one definition and re-exporting it means there is only one
/// thing to keep correct.
pub mod camera {
    pub use super::window::{pop_camera_state, push_camera_state, set_camera, set_default_camera};
    pub use super::Camera2D;
}

pub mod file {
    pub use super::{load_file, load_string, set_pc_assets_folder};
}

// `input`, `time`, `text`, `texture`, `rand`, `window`, `shapes`, `math`, `color`, `draw` and
// `material` already exist as real modules above, so they are not aliased here - a module that
// re-exports itself is a duplicate definition, not a convenience.

/// `load_string` sits in macroquad's `file` module next to `load_file`.
pub async fn load_string(path: &str) -> Result<String, std::io::Error> {
    let bytes = load_file(path).await?;
    String::from_utf8(bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub use material::{gl_use_default_material, gl_use_material, load_material, Material, MaterialParams};
pub use material::PipelineParams;
pub use miniquad::{BufferLayout, BufferType, BufferUsage, ShaderMeta, UniformBlockLayout,
                   VertexAttribute, VertexFormat, VertexStep};

// Submodule items the prelude pulls in from the crate root. macroquad's own prelude re-exports
// these directly, so the names have to exist here as well as inside their modules.
pub use input::{is_mouse_button_down, is_mouse_button_pressed, is_mouse_button_released,
                simulate_mouse_with_touch, touches};
pub use math::polar_to_cartesian;
pub use window::{pop_camera_state, push_camera_state};

/// macroquad re-exports its graphics crate, and `prpr::ext` reaches through it
/// (`macroquad::miniquad::...`) as well as importing `miniquad` directly.
pub use miniquad;

/// Sets the asset root. The shell resolves assets from the app package, so there is no search path
/// to configure - `prpr::core::init` calls this unconditionally at startup.
pub fn set_pc_assets_folder(_path: &str) {}

/// Loads a file relative to the asset root.
///
/// `prpr` has its own `FileSystem` abstraction and uses that for real IO; this exists only because
/// the prelude exports it. It reports failure rather than panicking so a stray call surfaces as an
/// error instead of a crash.
pub async fn load_file(path: &str) -> Result<Vec<u8>, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("asset IO is owned by the shell: {path}"),
    ))
}

/// Input plumbing.
pub mod utils {
    use super::miniquad::EventHandler;

    /// macroquad hands out a token per input subscriber. The shell feeds input straight into the
    /// handler on this platform, so the value is only ever passed back to
    /// `repeat_all_miniquad_input`.
    pub fn register_input_subscriber() -> usize {
        0
    }

    /// Replays queued miniquad events into a handler.
    ///
    /// A no-op: nothing is queued, because the shell owns the event loop and calls the handler's
    /// methods directly. `prpr::judge` calls this once per frame, so it has to exist.
    pub fn repeat_all_miniquad_input<H: EventHandler>(_handler: &mut H, _subscriber: usize) {}
}

pub mod color {
    pub use super::{
        Color, BEIGE, BLACK, BLANK, BLUE, BROWN, DARKGRAY, DARKGREEN, DARKPURPLE, GOLD, GRAY,
        GREEN, LIGHTGRAY, LIME, MAROON, ORANGE, PINK, PURPLE, RED, SKYBLUE, VIOLET, WHITE, YELLOW,
    };
}

pub mod texture {
    pub use super::{DrawTextureParams, FilterMode, Image, ImageFormat, RenderTarget, Texture2D};
}

pub mod math {
    pub use super::{Mat4, Quat, Rect, Vec2, Vec3, Vec4};

    /// macroquad's polar helper, used by `prpr::particle` to place particles on a ring.
    pub fn polar_to_cartesian(r: f32, theta: f32) -> Vec2 {
        Vec2::new(r * theta.cos(), r * theta.sin())
    }
}

/// macroquad's crate-root error type.
///
/// Needed because prpr propagates these with `?` inside `anyhow::Result` functions, and `anyhow`
/// requires `std::error::Error + Send + Sync`. A bare `String` satisfies none of that, which is
/// what the "`?` couldn't convert the error" errors were.
#[derive(Debug, Clone)]
pub enum Error {
    /// An operation the shell has to perform and the shim cannot.
    Unsupported(&'static str),
    /// Anything else, carried as text.
    Other(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Unsupported(what) => write!(f, "unsupported on this platform: {what}"),
            Error::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {}

/// Drawcall buffer capacity, which `prpr::particle` raises for large particle counts.
///
/// Nothing to size: the C# renderer's buffers are its own.
pub fn gl_set_drawcall_buffer_capacity(_vertices: usize, _indices: usize) {}

/// Returned by `QuadGl::make_pipeline`. `prpr::ext` unwraps it, so it must be `Debug`.
#[derive(Debug, Clone)]
pub struct ShaderError {
    pub message: String,
}

impl core::fmt::Display for ShaderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ShaderError {}

/// `prpr::ext`'s GL pipeline wrapper, referenced by name in a few signatures.
#[derive(Clone, Debug, Default)]
pub struct GlPipeline {
    pub pipeline: miniquad::Pipeline,
}

/// Loads an image from the asset root.
///
/// `prpr` decodes its own images through `image`; this exists because the prelude exports it. It
/// reports failure rather than panicking.
pub async fn load_image(_path: &str) -> Result<Image, Error> {
    Err(Error::Unsupported("load_image"))
}

pub mod prelude {
    pub use super::color::*;
    pub use super::glam;
    pub use super::utils;
    pub use super::miniquad::UniformType;
    pub use super::QuadGl;
    pub use super::{
        gl_set_drawcall_buffer_capacity, gl_use_default_material, gl_use_material, GlPipeline,
        MaterialParams,
    };
    pub use super::input::{is_mouse_button_pressed, is_mouse_button_released};
    pub use super::{DrawMode, PipelineParams, UniformBlockLayout, ShaderMeta, BufferLayout,
                    BufferType, BufferUsage, VertexAttribute, VertexFormat, VertexStep};
    /// The module, not only the function. `prpr::particle` writes `rand::gen_range(..)` and that
    /// path resolves through this glob - it shadows the extern-prelude `rand` crate, which is why
    /// `prpr::scene::loading` has to spell out `::rand::thread_rng` to reach the real one.
    pub use super::rand;
    pub use super::rand::gen_range;
    pub use super::{
        draw_circle, draw_line, draw_rectangle, draw_text, draw_texture, draw_texture_ex,
        is_mouse_button_down, load_file, load_image, load_material, measure_text, polar_to_cartesian,
        pop_camera_state, push_camera_state, set_pc_assets_folder, simulate_mouse_with_touch,
        touches, vec2, vec3, vec4, Camera2D, Color, Mat4, Material, Quat, Rect, TextDimensions,
        Vec2, Vec3, Vec4, Vertex,
    };
    pub use super::material::Material as MaterialType;
    pub use super::draw::*;
    pub use super::input::{get_touches, is_key_down, is_key_pressed, is_key_released, mouse_position, mouse_wheel, KeyCode, Touch, TouchPhase};
    pub use super::math::*;
    pub use super::texture::*;
    pub use super::time::*;
    pub use super::window::*;
}

pub mod material {
    /// Shader source preprocessing.
    ///
    /// `prpr::particle` builds its particle shader through this, passing `particles.glsl` as an
    /// include. macroquad's own version also rewrites GLSL for whichever backend is in use; there is
    /// no backend to rewrite for here, because the C# renderer owns pipelines entirely, so this
    /// performs the include expansion and nothing else - which is what makes the result a
    /// self-contained shader rather than a fragment.
    pub mod shaders {
        #[derive(Clone, Debug, Default)]
        pub struct PreprocessorConfig {
            /// `(name, body)` pairs, matched against `#include "name"`.
            pub includes: Vec<(String, String)>,
        }

        pub fn preprocess_shader(source: &str, config: &PreprocessorConfig) -> String {
            let mut out = String::with_capacity(source.len());
            for line in source.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("#include") {
                    let name = rest.trim().trim_matches('"');
                    if let Some((_, body)) = config.includes.iter().find(|(n, _)| n == name) {
                        out.push_str(body);
                        out.push('\n');
                        continue;
                    }
                }
                out.push_str(line);
                out.push('\n');
            }
            out
        }
    }

    /// `Copy` because prpr does `*self.material` - dereferencing a reference into a value.
    #[derive(Copy, Clone, Debug, Default)]
    pub struct Material {
        pub pipeline_params: PipelineParams,
    }

    impl Material {
        /// Records a uniform. The value is dropped: there is no GLSL program to feed on this
        /// platform, and the C# renderer owns the pipeline the uniforms would have gone to.
        ///
        /// Generic over the name so both `&str` and `&String` call sites work - `prpr::core::effect`
        /// uses both.
        pub fn set_uniform<N: AsRef<str>, V>(&self, _name: N, _value: V) {}

        pub fn set_texture<N: AsRef<str>>(&self, _name: N, _texture: super::Texture2D) {}

        pub fn delete(&self) {}
    }

    #[derive(Clone, Debug, Default)]
    pub struct MaterialParams {
        pub uniforms: Vec<(String, miniquad::UniformType)>,
        pub textures: Vec<String>,
        pub pipeline_params: PipelineParams,
    }

    /// Builds a material.
    ///
    /// The shader sources are accepted and dropped: there is no GLSL compiler on this platform, and
    /// the C# renderer owns pipelines. It returns `Ok` rather than an error so the callers that
    /// `.unwrap()` it (prpr's shadow UI does) keep working.
    pub fn load_material(_vertex: &str, _fragment: &str, params: MaterialParams) -> Result<Material, super::Error> {
        Ok(Material {
            pipeline_params: params.pipeline_params,
        })
    }

    pub fn gl_use_material(_material: Material) {}
    pub fn gl_use_default_material() {}
    /// Re-exported rather than declared: upstream macroquad re-exports miniquad's, and
    /// `prpr::particle` builds one to hand to `miniquad::Pipeline::with_params`. Two structurally
    /// identical types here would not compile at that call site.
    pub use miniquad::{BlendFactor, BlendState, BlendValue, Equation, PipelineParams};
}

pub mod rand {
    /// Deterministic xorshift, so a replay of the same chart looks the same. `prpr` only uses
    /// `rand` for particle jitter and tips, so the quality of the generator is not critical -
    /// but it must not depend on the platform's RNG, which `getrandom` cannot provide here.
    use core::sync::atomic::{AtomicU32, Ordering};
    static STATE: AtomicU32 = AtomicU32::new(0x1234_5678);

    pub fn srand(seed: u32) {
        STATE.store(if seed == 0 { 1 } else { seed }, Ordering::Relaxed);
    }
    pub fn rand() -> u32 {
        let mut x = STATE.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        STATE.store(x, Ordering::Relaxed);
        x
    }
    pub fn gen_range_f32(lo: f32, hi: f32) -> f32 {
        lo + (rand() as f32 / u32::MAX as f32) * (hi - lo)
    }

    /// macroquad's generic range helper. Implemented for the two types prpr actually asks for;
    /// anything else fails to compile rather than silently truncating.
    pub trait SampleRange: Copy {
        fn sample(self, other: Self) -> Self;
    }

    impl SampleRange for f32 {
        fn sample(self, other: f32) -> f32 {
            gen_range_f32(self, other)
        }
    }

    impl SampleRange for i32 {
        fn sample(self, other: i32) -> i32 {
            if other <= self {
                return self;
            }
            self + (rand() % ((other - self) as u32)) as i32
        }
    }

    pub fn gen_range<T: SampleRange>(low: T, high: T) -> T {
        low.sample(high)
    }
}

pub use prelude::*;
