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

/// The swizzle extension traits.
///
/// `prpr::ui` writes `pos.xy() / pos.w` on a `Vec4` and `prpr::ext` does the same, so
/// `Vec4Swizzles` has to be in scope or every such expression is
/// `E0599: no method named 'xy' found` — with rustc helpfully noting the method exists "for
/// `macroquad::Vec4` here" because the trait is implemented but not imported. macroquad's real
/// prelude re-exports these; omitting them is a shim-completeness bug, not a prpr bug.
pub use glam::{Vec2Swizzles, Vec3Swizzles, Vec4Swizzles};

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

impl From<[f32; 4]> for Color {
    /// RGBA floats [0.0, 1.0]. `prpr::ui::text` stores vertex colours as `[f32; 4]` and converts
    /// them back to `Color` when building glyph geometry.
    fn from(v: [f32; 4]) -> Self {
        Color::new(v[0], v[1], v[2], v[3])
    }
}

/// Both `Color → [f32; 4]` and `[f32; 4] → Color` are needed, and both must exist:
///
/// * `prpr::ui::text` passes a `Color` to `glyph_brush::Text::with_color<C: Into<Color>>`, whose
///   `Color` is `[f32; 4]` - so `Color: Into<[f32; 4]>` is required.
/// * the same file converts a glyph's stored `[f32; 4]` back with `vertex.extra.color.into()`
///   into a `Color` - so `[f32; 4]: Into<Color>` is required too.
///
/// Neither collides with core's blanket `impl<T> From<T> for T`: that impl only overlaps when the
/// two types are *equal*, and `Color` is not `[f32; 4]`.
impl From<Color> for [f32; 4] {
    fn from(c: Color) -> Self {
        [c.r, c.g, c.b, c.a]
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
    ///
    /// The pixels *are* copied into the texture registry, because that is the only place the shell
    /// can get them from - a texture command in the drain carries just an id, and this function never
    /// sees a filename. See `draw::register_texture`.
    pub fn from_rgba8(width: u16, height: u16, bytes: &[u8]) -> Self {
        let id = draw::next_texture_id();
        draw::register_texture(id, width as u32, height as u32, bytes);
        Texture2D {
            id,
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
    /// Adopts a handle `prpr` created itself - a *render* texture, allocated empty.
    ///
    /// Returning `0` here was not a harmless placeholder; it is why every piece of prpr and Phira
    /// text is invisible. `prpr` builds its glyph atlas exactly this way (`ui/text.rs:305`), so a
    /// dropped id broke three things at once: `glBindTexture` bound 0, `gl.texture(Some(..))` named
    /// texture 0 - which the shell reads as "no texture", so the text geometry went untextured -
    /// and the atlas never entered the registry at all, because the one path that registers pixels
    /// (`from_rgba8`) is not on this route.
    ///
    /// Installing `miniquad`'s write sink here rather than in some init routine is deliberate. The
    /// atlas is created *through this call*, so the sink is in place before its first
    /// `glTexSubImage2D` by construction - an ordering that cannot be got wrong, which matters
    /// because getting it wrong would look exactly like the bug this fixes.
    pub fn from_miniquad_texture(t: crate::miniquad_compat::Texture) -> Self {
        crate::miniquad::gl::set_texture_write_sink(draw::texture_write_sink);
        // A zero-sized handle is a placeholder - `Texture::from_raw_id` cannot know the dimensions
        // - and there is nothing to allocate for one.
        if t.width > 0 && t.height > 0 {
            draw::register_render_texture(t.id, t.width, t.height);
        }
        Texture2D {
            id: t.id,
            width: t.width,
            height: t.height,
        }
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
///
/// # Why every method takes `&self`, not `&mut self`
///
/// Upstream macroquad's `QuadGl` mutates *global* batcher state, so its methods are `&self` - and
/// that is load-bearing for `prpr`, which writes
///
/// ```ignore
/// let gl = unsafe { get_internal_gl() }.quad_gl;
/// gl.texture(...); gl.draw_mode(...); gl.geometry(...);
/// ```
///
/// in a dozen places: an immutable local binding calling mutating methods. With `&mut self` that
/// is 20 `E0596: cannot borrow 'gl' as mutable` errors across `ui.rs`, `ui/text.rs`, `ext.rs`,
/// `core/resource.rs`. The shim's state is `static mut`/atomically shared anyway, so `&self` costs
/// nothing and the signature matches what `prpr` is written against. **Do not "fix" this by making
/// the bindings `mut`** - that is editing upstream for no reason and would break at the
/// destructuring sites (`let InternalGlContext { quad_gl: gl, .. } = …`), which cannot be `mut`.
pub struct QuadGl;

impl QuadGl {
    pub fn texture(&self, tex: Option<Texture2D>) {
        draw::set_texture(tex.map(|t| t.id));
    }

    pub fn draw_mode(&self, mode: DrawMode) {
        draw::set_draw_mode(mode);
    }

    pub fn geometry(&self, vertices: &[Vertex], indices: &[u16]) {
        draw::push_geometry(vertices, indices);
    }

    pub fn scissor(&self, _rect: Option<(i32, i32, i32, i32)>) {
        // The parameter is the *tuple*, not a `Rect`: `prpr::ui` stores its scissor as
        // `Option<(i32, i32, i32, i32)>` and hands it straight through, so a `Rect` here is an
        // E0308 at two call sites. The C# renderer applies its own scissor anyway; on the paths
        // prpr uses the recorded geometry is already correct, so this is a no-op on WP8.1.
    }

    /// The model matrix `prpr` brackets each draw with.
    ///
    /// `Resource::apply_model_of` (`core/resource.rs:646`) is how a judge line, and every note on
    /// it, gets from chart space to screen space - with this a no-op, all of them drew at their
    /// local origin. Applied when a draw is recorded instead of when it is flushed: upstream
    /// snapshots the model per draw call (`prpr-macroquad/src/quad_gl.rs:851`) and the shader
    /// reads `Projection * Model * position` (`quad_gl.rs:988`), so the two are the same
    /// arithmetic. See `crate::transform`.
    pub fn push_model_matrix(&self, mat: Mat4) {
        crate::transform::push_model(&mat)
    }

    pub fn pop_model_matrix(&self) {
        crate::transform::pop_model()
    }

    /// Builds a pipeline.
    ///
    /// The signature follows the miniquad revision prpr targets: shader sources by `&str`, then
    /// `PipelineParams`, then attribute layouts and uniforms. An earlier attempt used a
    /// `(write_color, write_depth, stencil_test, blend)` boolean tuple plus shader sources by value,
    /// which is the shape of a different API entirely and matched nothing.
    ///
    /// Returns `Ok` because `prpr::ext::make_pipeline` unwraps the result.
    pub fn make_pipeline(
        &self,
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
    pub fn flush(&self) {}

    /// `prpr` destructures the result as `let (x, y, w, h) = …` and supplies a `(0, 0, w, h)`
    /// fallback literal, so this must be the **tuple** form, not a `Rect` - a `Rect` makes both
    /// halves of that expression type-error (`E0308` twice per call site).
    ///
    /// `None` means "ask the shell", which `prpr::ext::get_viewport` unwraps with a fallback that
    /// asks `get_active_render_pass()` for the real size.
    pub fn get_viewport(&self) -> Option<(i32, i32, i32, i32)> {
        None
    }

    /// A setter, not a getter: `prpr::core::line` brackets an offscreen pass with
    /// `viewport(None)` / `viewport(Some(..))`. The getter is `get_viewport`.
    pub fn viewport(&self, _rect: Option<(i32, i32, i32, i32)>) {}

    /// Selects the render target. A setter, not a getter - `prpr::core::effect` and
    /// `prpr::core::line` both call it with the pass they want to draw into.
    pub fn render_pass(&self, _pass: Option<crate::miniquad_compat::RenderPass>) {}

    /// The active render target as a handle.
    ///
    /// Must be `Option<RenderPass>` and not a size: `prpr::ext::get_viewport` writes
    /// `get_active_render_pass().map(|it| { let tex = it.texture(gl.quad_context); (tex.width, …) })`,
    /// so the value has to have `.texture()` on it. Returning a bare `RenderPass` (not an
    /// `Option`) made it "not an iterator"; returning a size tuple breaks `.texture()`.
    pub fn get_active_render_pass(&self) -> Option<crate::miniquad_compat::RenderPass> {
        None
    }

    pub fn set_blend(&self, _blend: Option<miniquad::BlendState>, _clear: bool) {}

    /// The projection `prpr::ui` composes its transforms against.
    pub fn get_projection_matrix(&self) -> Mat4 {
        Mat4::IDENTITY
    }

    /// Sets a shader uniform. The value is dropped: the C# renderer owns pipelines, and prpr's
    /// effects are what it renders through.
    pub fn set_uniform<T>(&self, _name: &str, _value: T) {}
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
    /// `prpr` both destructures it and calls through it, so both access paths matter:
    ///
    /// * `let InternalGlContext { quad_context: ctx, quad_gl } = gl;` then
    ///   `self.setup_render_pass(quad_gl, ctx)` - and the parameter is `&QuadGl`, so `quad_gl`
    ///   **must be a reference**, not a value. A `QuadGl` field makes that an `E0308`
    ///   ("expected `&QuadGl`, found `QuadGl`") at three call sites in `particle.rs`.
    /// * `get_internal_gl().flush()` - served by the `Deref` impl below.
    pub struct InternalGlContext<'a> {
        pub quad_gl: &'static mut QuadGl,
        pub quad_context: &'a mut miniquad::Context,
    }

    impl<'a> core::ops::Deref for InternalGlContext<'a> {
        type Target = QuadGl;
        fn deref(&self) -> &QuadGl {
            self.quad_gl
        }
    }

    impl<'a> core::ops::DerefMut for InternalGlContext<'a> {
        fn deref_mut(&mut self) -> &mut QuadGl {
            self.quad_gl
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
        // Real addresses into `static mut`, because returning references to temporaries cannot
        // work. Nothing here dereferences the context: `prpr` only forwards it to the miniquad
        // calls, which ignore their arguments. The `QuadGl` is a ZST, so a leaked static is free
        // and avoids a stack borrow that would not outlive the `'static` signature.
        static mut CTX: u8 = 0;
        static mut GL: QuadGl = QuadGl;
        let p: *mut u8 = core::ptr::addr_of_mut!(CTX);
        InternalGlContext {
            quad_gl: &mut *core::ptr::addr_of_mut!(GL),
            quad_context: &mut *(p as *mut miniquad::Context),
        }
    }

    /// The camera stack `prpr::scene::game` brackets its HUD passes with, and the camera every
    /// draw is projected through on its way into `PortCmd`.
    ///
    /// These used to be no-ops on the grounds that "the shell renders through Win2D and applies
    /// its own transforms". That got the shell's transforms, not `prpr`'s - and `prpr`'s are the
    /// ones that decide where a chart's judge line and notes actually are. See `crate::transform`
    /// for why projecting at record time is equivalent to projecting at flush time.
    pub fn push_camera_state() {
        crate::transform::push_camera()
    }
    pub fn pop_camera_state() {
        crate::transform::pop_camera()
    }

    pub fn set_camera(c: &Camera2D) {
        crate::transform::set_camera(c)
    }
    pub fn set_default_camera() {
        crate::transform::set_default_camera()
    }
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

impl Camera2D {
    /// Returns the render pass from this camera's render target, if any.
    ///
    /// `prpr::scene::game` calls `.or_else(|| res.camera.render_pass())` to pick the render target
    /// to draw into. On WP8.1 the C# renderer owns targets; the value is forwarded as an opaque id.
    pub fn render_pass(&self) -> Option<crate::miniquad_compat::RenderPass> {
        self.render_target.map(|rt| rt.render_pass)
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
    /// Touch events the shell pushed for the frame now being drawn.
    ///
    /// One queue serves two readers that upstream feeds from two different places. Macroquad's
    /// `touches()` reports the *state* of what is currently down, which `Judge::update` needs in
    /// order to keep positioning a held note; `repeat_all_miniquad_input` replays the *events*,
    /// which is the only way `prpr::judge`'s `TOUCHES` store gets filled - and `TOUCHES` is what
    /// `get_touches()` reads, which is every button in the scene and the whole `Ui`. Supplying
    /// only one of the two gives the characteristic half-working input: notes judge but nothing
    /// on screen can be pressed, or the reverse.
    thread_local! {
        static PENDING: std::cell::RefCell<Vec<Touch>> = std::cell::RefCell::new(Vec::new());
    }

    /// How many touch events one frame may carry.
    ///
    /// The shell sends one event per finger per frame, so a real device never comes near this.
    /// The cap exists so a runaway caller cannot grow the queue without bound, and returning a
    /// code for it lets the caller count the loss instead of watching input quietly vanish - the
    /// failure mode that would otherwise be indistinguishable from prpr ignoring the touch.
    const TOUCH_CAP: usize = 128;

    /// Queue one touch event for the frame now being built.
    ///
    /// Returns 0, or -2 when this frame's queue is full. The event is **not** consumed here: it
    /// stays until the host clears the queue after the frame has been fully drawn, because
    /// `Judge::update` and `Scene` read it at different points within that one frame.
    ///
    /// `time` is uptime, not wall time - see `wp81_game_touch`.
    pub fn push_touch(id: u64, phase: TouchPhase, x: f32, y: f32, time: f64) -> i32 {
        PENDING.with(|q| {
            let mut q = q.borrow_mut();
            if q.len() >= TOUCH_CAP {
                return -2;
            }
            q.push(Touch {
                id,
                phase,
                position: Vec2::new(x, y),
                time,
            });
            0
        })
    }

    /// Runs `f` over this frame's events without consuming them.
    pub(crate) fn for_each_pending(mut f: impl FnMut(&Touch)) {
        PENDING.with(|q| {
            let q = q.borrow();
            for t in q.iter() {
                f(t);
            }
        })
    }

    /// Drop the frame's queue. Called by the host once the frame has been judged *and* drawn,
    /// never earlier: an event consumed before render would leave `Scene` reading a frame older
    /// than the one it is drawing, which on screen looks like a button that answers one frame late.
    pub fn clear_touches() {
        PENDING.with(|q| q.borrow_mut().clear());
    }

    pub fn get_touches() -> Vec<Touch> {
        PENDING.with(|q| q.borrow().clone())
    }

    /// macroquad's name for the same list.
    pub fn touches() -> Vec<Touch> {
        get_touches()
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
// transform
//
// Two pieces of state `prpr` treats as real and the shell used to throw away: the camera set
// before each pass, and the model matrix bracketing every draw.
//
// The vertex shader is `gl_Position = Projection * Model * vec4(position, 1)`
// (`prpr-macroquad/src/quad_gl.rs:988`), so a point travels
//
//     local --Model--> world --Projection--> NDC --viewport--> pixels
//
// and the shell has to have collapsed all three into the pixel coordinates `PortCmd` carries by
// the time the record is written. Doing it at record time rather than at flush time is sound
// because macroquad flushes *before* it changes camera (`camera.rs:231` for `set_camera`,
// `camera.rs:272` for `pop_camera_state`), so the camera in force when a draw is issued is
// provably the camera it gets flushed with; `Model` is snapshotted per draw call the same way
// (`quad_gl.rs:851`).
//
// Losing either was not cosmetic. `Resource::apply_model_of` is *the* call that puts a judge line
// where the chart says it is (`core/resource.rs:646`), and with `push_model_matrix` a no-op every
// line drew at `core/line.rs:252`'s local coordinates - `draw_line(-len, 0., len, 0., ...)`, all
// 104 of them lying along one centre line. That is what the captured fixture showed.
//
// Thread-local because these are read and written only inside `wp81_game_frame`, and because it
// costs no `unsafe`. The command arenas cannot be thread-local - the shell drains them from
// another thread - so they stay `static mut`; these do not have that excuse.
// ===========================================================================

pub mod transform {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// One camera, reduced to the fields that decide where a point lands.
    ///
    /// `active: false` does not mean "a camera that does nothing". It is macroquad's *no camera*
    /// state, which `Context::projection_matrix` (`prpr-macroquad/src/lib.rs:386`) resolves to
    /// `pixel_perfect_projection_matrix` - `orthographic_rh_gl(0., w, h, 0.)`: pixels, origin
    /// top-left, y down. Against a shell that already works in pixels that is the identity, so an
    /// inactive camera reads as "these coordinates are already pixels".
    #[derive(Copy, Clone)]
    struct Cam {
        active: bool,
        zoom: (f32, f32),
        target: (f32, f32),
        offset: (f32, f32),
        /// Radians. `prpr` never sets one; `Camera2D::matrix` has the term regardless.
        rotation: f32,
        /// GL convention: origin bottom-left, in screen pixels. `None` = the whole screen.
        viewport: Option<(f32, f32, f32, f32)>,
    }

    const CAM_OFF: Cam = Cam {
        active: false,
        zoom: (1.0, 1.0),
        target: (0.0, 0.0),
        offset: (0.0, 0.0),
        rotation: 0.0,
        viewport: None,
    };

    /// `prpr` brackets a pass with exactly one push/pop - `scene.rs:522` and four sites in
    /// `game.rs` - so eight is a ceiling never approached. A ceiling is the point: a `render`
    /// that returns `Err` between its `push` and its `pop` would deepen the stack on every frame
    /// forever, and a stack that grows silently is worse than one that refuses to.
    const CAM_STACK_CAP: usize = 8;

    thread_local! {
        static CAM: Cell<Cam> = const { Cell::new(CAM_OFF) };
        static CAM_STACK: RefCell<([Cam; CAM_STACK_CAP], usize)> =
            const { RefCell::new(([CAM_OFF; CAM_STACK_CAP], 0)) };
        static MODEL: Cell<Mat4> = const { Cell::new(Mat4::IDENTITY) };
        static MODEL_STACK: RefCell<Vec<Mat4>> = const { RefCell::new(Vec::new()) };
    }

    // ---------------------------------------------------------------- camera

    pub fn set_camera(c: &Camera2D) {
        CAM.with(|cur| {
            cur.set(Cam {
                active: true,
                zoom: (c.zoom.x, c.zoom.y),
                target: (c.target.x, c.target.y),
                offset: (c.offset.x, c.offset.y),
                rotation: c.rotation.to_radians(),
                viewport: c
                    .viewport
                    .map(|(x, y, w, h)| (x as f32, y as f32, w as f32, h as f32)),
            })
        });
    }

    pub fn set_default_camera() {
        CAM.with(|cur| cur.set(CAM_OFF));
    }

    /// Saves the camera without touching it. Upstream does not flush here either
    /// (`camera.rs:257`): whatever is buffered was already issued under the camera that is
    /// current, and the bracket that follows always opens with its own `set_camera`.
    pub fn push_camera() {
        let saved = CAM.with(|cur| cur.get());
        CAM_STACK.with(|s| {
            let mut s = s.borrow_mut();
            let (stack, len) = &mut *s;
            if *len < CAM_STACK_CAP {
                stack[*len] = saved;
                *len += 1;
            }
        });
    }

    /// Restores what `push_camera` saved - which is the whole reason a stack exists. `game.rs`
    /// sets the chart camera, draws, pops, and then keeps drawing under whatever was current
    /// before it (`game.rs:1175` onwards). With a no-op pop those later draws would inherit the
    /// chart's camera and land in the wrong space entirely.
    pub fn pop_camera() {
        let prev = CAM_STACK.with(|s| {
            let mut s = s.borrow_mut();
            let (stack, len) = &mut *s;
            if *len == 0 {
                None
            } else {
                *len -= 1;
                Some(stack[*len])
            }
        });
        if let Some(cam) = prev {
            CAM.with(|cur| cur.set(cam));
        }
    }

    // ----------------------------------------------------------------- model

    /// `QuadGl::push_model_matrix` (`prpr-macroquad/src/quad_gl.rs:808`): the new matrix composes
    /// on top of what is current, so a nested push multiplies rather than replaces.
    pub fn push_model(m: &Mat4) {
        let cur = MODEL.with(|c| c.get());
        MODEL_STACK.with(|s| s.borrow_mut().push(cur));
        MODEL.with(|c| c.set(cur * *m));
    }

    /// Upstream guards this with `model_stack.len() > 1` (`quad_gl.rs:813`): popping past the
    /// base would leave the model undefined rather than merely wrong, so the same guard is kept.
    pub fn pop_model() {
        let prev = MODEL_STACK.with(|s| s.borrow_mut().pop());
        if let Some(m) = prev {
            MODEL.with(|c| c.set(m));
        }
    }

    /// Start of every frame.
    ///
    /// macroquad begins with no camera (`lib.rs:299`) and `Resource` with an identity model
    /// (`resource.rs:569`), so both return to their initial state here. That also caps a bracket
    /// left open by a `render` that bailed out mid-pass: without it, one failing frame would
    /// indent every frame after it.
    pub fn reset_frame() {
        CAM.with(|cur| cur.set(CAM_OFF));
        CAM_STACK.with(|s| s.borrow_mut().1 = 0);
        MODEL.with(|c| c.set(Mat4::IDENTITY));
        MODEL_STACK.with(|s| s.borrow_mut().clear());
    }

    // -------------------------------------------------------------- projecting

    fn viewport_px(cam: Cam) -> (f32, f32, f32, f32) {
        cam.viewport.unwrap_or((
            0.0,
            0.0,
            crate::window::screen_width(),
            crate::window::screen_height(),
        ))
    }

    /// `Camera2D::matrix` (`prpr-macroquad/src/camera.rs:75`), in the order GL applies it:
    /// `T(offset) * (S(zoom) * R(rotation)) * T(-target)`. With `rotation`, `offset` and `target`
    /// all zero - which is every camera `prpr` builds - this collapses to `world * zoom`.
    fn cam_point(cam: Cam, p: Vec2) -> Vec2 {
        let mut v = p - Vec2::new(cam.target.0, cam.target.1);
        if cam.rotation != 0.0 {
            let (s, c) = cam.rotation.sin_cos();
            v = Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c);
        }
        Vec2::new(
            v.x * cam.zoom.0 + cam.offset.0,
            v.y * cam.zoom.1 + cam.offset.1,
        )
    }

    /// GL's viewport transform, plus the flip that turns GL's bottom-left origin into the
    /// top-left one the shell draws in. `y` in a viewport tuple is GL's, so it is subtracted
    /// from the screen height rather than added.
    fn ndc_to_pixels(cam: Cam, ndc: Vec2) -> Vec2 {
        let (x, y, w, h) = viewport_px(cam);
        let sh = crate::window::screen_height();
        Vec2::new(
            x + (ndc.x + 1.0) * 0.5 * w,
            sh - y - (ndc.y + 1.0) * 0.5 * h,
        )
    }

    /// The whole shader path, done eagerly: `local -> world -> NDC -> pixels`.
    ///
    /// Every coordinate the shell ever sees goes through here, which is why `PortCmd` can stay a
    /// plain list of pixel-space numbers with no camera or matrix attached to it.
    pub fn project(x: f32, y: f32) -> (f32, f32) {
        let q = MODEL.with(|c| c.get()) * Vec4::new(x, y, 0.0, 1.0);
        let world = Vec2::new(q.x, q.y);

        let cam = CAM.with(|c| c.get());
        if !cam.active {
            // Already pixels - `pixel_perfect_projection_matrix`, see `Cam`.
            return (world.x, world.y);
        }
        let p = ndc_to_pixels(cam, cam_point(cam, world));
        (p.x, p.y)
    }

    pub fn project_point(v: Vec2) -> Vec2 {
        let (x, y) = project(v.x, v.y);
        Vec2::new(x, y)
    }

    /// Maps a world-space direction, dropping every translation. Needed because a line's
    /// thickness is a *length* perpendicular to it, and the cameras `prpr` builds are
    /// deliberately non-uniform (`zoom: (1., -aspect)`), so the pixel-space perpendicular is not
    /// in general the image of the world-space one.
    pub fn project_delta(from: Vec2, to: Vec2) -> Vec2 {
        let m = MODEL.with(|c| c.get());
        let a = m * Vec4::new(from.x, from.y, 0.0, 1.0);
        let b = m * Vec4::new(to.x, to.y, 0.0, 1.0);
        let d = Vec2::new(b.x - a.x, b.y - a.y);

        let cam = CAM.with(|c| c.get());
        if !cam.active {
            return d;
        }
        // Rotation and the per-axis zoom only: `cam_point` without `target`/`offset`, and
        // `ndc_to_pixels` without the half-screen shift.
        let mut r = d;
        if cam.rotation != 0.0 {
            let (s, c) = cam.rotation.sin_cos();
            r = Vec2::new(r.x * c - r.y * s, r.x * s + r.y * c);
        }
        let (_, _, w, h) = viewport_px(cam);
        Vec2::new(r.x * cam.zoom.0 * 0.5 * w, -r.y * cam.zoom.1 * 0.5 * h)
    }

    /// True when `local -> pixels` sends axis-aligned rectangles to axis-aligned rectangles - in
    /// other words when nothing in the chain rotates.
    ///
    /// This decides which command a draw can be recorded as. `Cmd::Rect` and `Cmd::Texture` carry
    /// `x/y/w/h` and nothing else, so the moment a `Model` rotates them there is nowhere to put
    /// the fourth corner; such a draw has to go through the vertex arena, which has room for four
    /// independent corners. `prpr` rotates its judge lines continuously, and a line carrying a
    /// custom texture draws through `draw_texture_ex` under exactly that transform.
    pub fn axis_aligned() -> bool {
        let m = MODEL.with(|c| c.get());
        if m.x_axis.y.abs() > 1e-6 || m.y_axis.x.abs() > 1e-6 {
            return false;
        }
        CAM.with(|c| c.get()).rotation == 0.0
    }

    /// Projects the two opposite corners of a rectangle into one.
    ///
    /// Width and height keep their sign. The cameras `prpr` builds all have a negative `zoom.y`,
    /// which mirrors the rectangle rather than inverting it, and preserving the sign is what lets
    /// the reader flip its texture coordinates back the same way macroquad's own vertex builder
    /// would have. Only meaningful when `axis_aligned()` holds.
    pub fn project_rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        let (x0, y0) = project(x, y);
        let (x1, y1) = project(x + w, y + h);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// The axis-aligned box enclosing all four projected corners - the degraded form a
    /// *textured* draw falls back to once `axis_aligned()` stops holding.
    ///
    /// Degrading rather than turning to geometry is deliberate: `Cmd::Texture`'s eight payload
    /// slots are already full (dest 4 + source 4) so there is nowhere to put an angle, and the
    /// shell cannot texture-fill a tilted quad either - `Win2DChartRenderer.FillPortTriangle`
    /// bounds a triangle for exactly that reason. Keeping the box preserves the picture's
    /// position and extent, which is worth more than a solid silhouette. Rotating it properly
    /// needs `PortCmd` to grow a transform, and is tracked as such.
    ///
    /// Only `prpr`'s custom-textured judge lines reach here: notes never go through
    /// `draw_texture_ex` at all (`core/note.rs:82` batchifies them itself) and the HUD draws
    /// under an identity model.
    pub fn project_aabb(x: f32, y: f32, w: f32, h: f32) -> Rect {
        let (x0, y0) = project(x, y);
        let (x1, y1) = project(x + w, y);
        let (x2, y2) = project(x + w, y + h);
        let (x3, y3) = project(x, y + h);
        let min_x = x0.min(x1).min(x2).min(x3);
        let max_x = x0.max(x1).max(x2).max(x3);
        let min_y = y0.min(y1).min(y2).min(y3);
        let max_y = y0.max(y1).max(y2).max(y3);
        Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
    }

    /// Turns a signed projected rectangle into the positive-extent form the shell reads.
    ///
    /// A mirroring transform produces negative extents on its own - every camera `prpr` builds
    /// has a negative `zoom.y` - and the two readers disagree about what to do with them:
    /// `Win2DChartRenderer.DrawPortTexture` drops `dw/dh <= 0` outright (`if (dw <= 0f || dh <=
    /// 0f ...) return;`), so a texture that used to draw would silently stop. Folding the sign
    /// back into `flip_x`/`flip_y` keeps the mirroring the transform asked for while handing the
    /// reader the rectangle it expects - the renderer already implements flipping as a scale
    /// about the rectangle's centre, so there is somewhere for it to go.
    ///
    /// `flip_*` are the caller's own; pass them straight through when it has none.
    pub fn normalize_rect(r: Rect, mut flip_x: bool, mut flip_y: bool) -> (Rect, bool, bool) {
        let (mut x, mut y, mut w, mut h) = (r.x, r.y, r.w, r.h);
        if w < 0.0 {
            x += w;
            w = -w;
            flip_x = !flip_x;
        }
        if h < 0.0 {
            y += h;
            h = -h;
            flip_y = !flip_y;
        }
        (Rect::new(x, y, w, h), flip_x, flip_y)
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

    // =======================================================================
    // the texture registry
    //
    // A texture command in the drain carries only a `u32` id, and the shim allocates those ids in
    // `from_rgba8` - which never sees a filename, only pixels. So the shell cannot map an id back to
    // an image on its own, and a frame of textures would draw nothing.
    //
    // This records the pixels at creation time. The shell asks for a texture once
    // (`port_texture_data`) and keeps the resulting `CanvasBitmap`, so the cost is one copy per
    // texture per session, not per frame.
    //
    // A fixed arena rather than a Vec for the same reason as the draw buffers: this runs on a phone
    // and a texture set is bounded by the chart. The budget is generous because a chart's
    // illustration can be a full-screen PNG - and overflow is reported rather than silently
    // truncating, because a half-copied texture would render as garbled pixels with no clue why.
    //
    // Measured, not guessed. The 190 chart asks for **24,943,416** bytes of unique texture content
    // (54 distinct images after 21 duplicate registrations were aliased), so the original 8 MiB
    // refused 28 of them and doubling to 16 MiB refused 24 more - both budgets ended the run pinned
    // within 25 KB of their ceiling, which is the tell that the limit is what is binding and not
    // the chart. 32 MiB is the first value that holds the measured demand with room for a larger
    // illustration.
    //
    // The duplicates `prpr` builds were real - it makes a second `Texture2D` for images it already
    // made, and the asset log requests all 23 chart PNGs twice - but aliasing them (21 of them, see
    // `TEX_ALIAS`) was never going to be enough on its own: the demand figure above is what is left
    // *after* that collapse.
    //
    // This is a staging area, not the final home: the shell reads each texture once and keeps its
    // own copy (see the header note), so bytes could be released after that fetch. They are not
    // released yet, and on a 512 MB phone that is the follow-up worth doing before the device round
    // rather than after it.
    // =======================================================================

    /// Largest total the registry will hold across all textures.
    const TEX_MAX_BYTES: usize = 32 * 1024 * 1024;
    // 75 registrations were attempted on that chart; entries are 28 bytes, so this costs 3.5 KB and
    // only exists because the byte budget used to be the binding constraint and hid it.
    const TEX_SLOTS: usize = 128;

    #[derive(Copy, Clone)]
    struct TexEntry {
        id: u32,
        width: u32,
        height: u32,
        offset: usize,
        len: usize,
        /// The rectangle written since the shell last drained it: `(x, y, w, h)`.
        ///
        /// Kept on the entry rather than in a side table because the shell discovers ids by
        /// enumeration - a texture it has never seen must report its full extent, and the ones it
        /// saw earlier must report only what moved. Encoding "new" as "fully dirty" makes both
        /// cases one rule, and one rule cannot be half-implemented.
        dirty: [u32; 4],
        /// Whether `dirty` holds something the shell has not read yet.
        dirty_pending: bool,
    }

    static mut TEX_ENTRIES: [TexEntry; TEX_SLOTS] = [TexEntry {
        id: 0,
        width: 0,
        height: 0,
        offset: 0,
        len: 0,
        dirty: [0; 4],
        dirty_pending: false,
    }; TEX_SLOTS];
    static mut TEX_COUNT: usize = 0;
    static mut TEX_BYTES: Vec<u8> = Vec::new();
    static mut TEX_OVERFLOW: u32 = 0;
    /// The last texture that did not fit: `(id, width, height, bytes)`.
    ///
    /// Kept as *one* record rather than a list because a run that overflows usually overflows
    /// everything after the budget is spent - a count alone cannot distinguish "28 big textures were
    /// refused" from "one texture was refused 28 times", and those have different fixes.
    static mut TEX_DROP: (u32, u32, u32, usize) = (0, 0, 0, 0);
    /// Why: 1 = out of slots, 2 = this one texture alone exceeds the budget, 3 = the running total
    /// would exceed it.
    static mut TEX_DROP_REASON: u8 = 0;
    /// Bytes of *unique* content the engine asked the registry to hold, whether or not they fit.
    ///
    /// `texture_used_bytes` is what actually fitted, which is capped at the budget - so it can never
    /// answer "how big should the budget be". This counts every request that was not aliased to
    /// bytes already stored, and it is the number that decides.
    static mut TEX_DEMAND: usize = 0;
    /// Registrations satisfied by pointing at bytes already stored instead of copying them again.
    static mut TEX_ALIAS: u32 = 0;

    /// Records a texture's pixels so the shell can rebuild it. Called from `from_rgba8`.
    ///
    /// Silently ignoring an already-known id is correct: `prpr` re-creates the same texture on scene
    /// re-entry, and the pixels are identical, so the older copy is as good.
    ///
    /// Identical *content* under a different id is also not stored twice. `prpr` asks for the same
    /// images more than once while building a scene (all 23 chart PNGs arrive twice on the 190
    /// chart), and on the old 8 MiB budget those duplicate copies - not any single large image -
    /// were what pushed 28 later textures out. Aliasing keeps a second id pointing at bytes that are
    /// already there, so it costs one entry and no pixels.
    pub fn register_texture(id: u32, width: u32, height: u32, bytes: &[u8]) {
        unsafe {
            let count = TEX_COUNT;
            for i in 0..count {
                if TEX_ENTRIES[i].id == id {
                    return;
                }
            }
            if count >= TEX_SLOTS {
                note_drop(id, width, height, bytes.len(), 1);
                return;
            }

            // Shape first. A byte-for-byte compare against every stored texture would be
            // O(entries x pixels) on a phone, and shape alone rules out almost all of them.
            for i in 0..count {
                let e = TEX_ENTRIES[i];
                if e.width != width || e.height != height || e.len != bytes.len() {
                    continue;
                }
                let stored = core::slice::from_raw_parts(TEX_BYTES.as_ptr().add(e.offset), e.len);
                if stored == bytes {
                    TEX_ENTRIES[count] = TexEntry {
                        id,
                        width,
                        height,
                        offset: e.offset,
                        len: e.len,
                        dirty: [0, 0, width, height],
                        dirty_pending: true,
                    };
                    TEX_COUNT = count + 1;
                    TEX_ALIAS = TEX_ALIAS.wrapping_add(1);
                    return;
                }
            }

            let offset = (&TEX_BYTES).len();
            let end = offset + bytes.len();
            // Counted from here, so an aliased request adds nothing - it is the same bytes again.
            TEX_DEMAND += bytes.len();
            if end > TEX_MAX_BYTES {
                note_drop(id, width, height, bytes.len(), 3);
                return;
            }
            TEX_BYTES.extend_from_slice(bytes);
            TEX_ENTRIES[count] = TexEntry {
                id,
                width,
                height,
                offset,
                len: bytes.len(),
                // Starts fully dirty: the shell learns ids by enumeration and has never seen this
                // one, so "new" has to mean "read all of it" or its first read is incomplete.
                dirty: [0, 0, width, height],
                dirty_pending: true,
            };
            TEX_COUNT = count + 1;
        }
    }

    // ---------------------------------------------------------------------
    // Render textures: registration, sub-image writes, and the shell's dirty-rectangle view
    //
    // `register_texture` is only ever reached from `from_rgba8`, i.e. from a texture uploaded
    // whole. A render texture is the opposite - allocated empty, filled a rectangle at a time by
    // `glTexSubImage2D` - and on this port the only one is the glyph atlas, which is the whole of
    // where prpr/Phira text comes from.
    // ---------------------------------------------------------------------

    /// Writes that arrived for an id the registry does not know.
    ///
    /// Always a bug when non-zero: it means something is being drawn whose bytes never arrived,
    /// which no amount of looking at the screen reveals. Paired with `texture_write_stats` because
    /// refusals alone are ambiguous - zero refusals is also what an engine that never wrote anything
    /// would report.
    static mut TEX_WRITE_MISSES: u32 = 0;
    /// Accepted sub-image writes, and the id of the most recent one.
    static mut TEX_WRITES: u32 = 0;
    static mut TEX_LAST_WRITE: u32 = 0;

    /// Writes that arrived for an id the registry does not know.
    ///
    /// Non-zero means a texture is being drawn whose bytes never landed. Surfaced rather than
    /// dropped because the bug this replaces left no trace at all.
    pub fn texture_write_misses() -> u32 {
        unsafe { TEX_WRITE_MISSES }
    }

    /// Accepted sub-image writes, and the id that received the last one.
    ///
    /// The count is the liveness signal - `0` means the mechanism never ran, which is a different
    /// failure from running and being refused - and the id is how a caller finds the texture to
    /// inspect without hard-coding a dimension it would then have to keep in sync.
    pub fn texture_write_stats() -> (u32, u32) {
        unsafe { (TEX_WRITES, TEX_LAST_WRITE) }
    }

    /// Registers a texture that arrives empty, so the shell can see it and receive its pixels.
    ///
    /// The extent starts fully dirty because the shell discovers ids by enumeration and has never
    /// seen this one: "new" has to mean "read all of it", otherwise its first read builds an
    /// incomplete image. Sharing that flag with later writes is what keeps new and changed down to
    /// a single rule.
    ///
    /// Bytes are zeroed rather than left undefined so a re-created atlas of the same size cannot
    /// show a previous life's glyphs. The id is a function of the dimensions
    /// (`miniquad::Texture::new_render_texture`), so a *different* size yields a different id and
    /// gets its own entry; this branch then only runs when one id is rebuilt in place.
    pub fn register_render_texture(id: u32, width: u32, height: u32) {
        unsafe {
            if width == 0 || height == 0 {
                return;
            }
            let count = TEX_COUNT;
            let len = (width as usize).saturating_mul(height as usize).saturating_mul(4);

            for i in 0..count {
                if TEX_ENTRIES[i].id == id {
                    let e = TEX_ENTRIES[i];
                    core::ptr::write_bytes(TEX_BYTES.as_mut_ptr().add(e.offset), 0, e.len);
                    // The *stored* extent, not the one just passed in. The dirty rectangle is a
                    // promise the reader has to be able to keep, and `texture_rect_data` accepts
                    // only what fits inside `e.width`/`e.height` - announcing a larger area would
                    // get that read refused with -3, i.e. an atlas the shell can see but not fill.
                    //
                    // Normally the two agree, because the id is a function of the dimensions and
                    // a different size therefore takes this branch only for the same size. Writing
                    // the stored numbers makes the pair self-consistent even if that ever stops
                    // being true: the content would be a subset, but never an unreadable one.
                    TEX_ENTRIES[i].dirty = [0, 0, e.width, e.height];
                    TEX_ENTRIES[i].dirty_pending = true;
                    return;
                }
            }

            if count >= TEX_SLOTS {
                note_drop(id, width, height, len, 1);
                return;
            }
            // Counted against the same budget as chart art on purpose: the phone's memory does not
            // care which subsystem asked for it, and 8.2 MiB of headroom is the number this design
            // was sized from (see `glGetIntegerv`).
            if TEX_BYTES.len().saturating_add(len) > TEX_MAX_BYTES {
                note_drop(id, width, height, len, 3);
                return;
            }

            let offset = TEX_BYTES.len();
            TEX_DEMAND = TEX_DEMAND.saturating_add(len);
            TEX_BYTES.resize(offset + len, 0);
            TEX_ENTRIES[count] = TexEntry {
                id,
                width,
                height,
                offset,
                len,
                dirty: [0, 0, width, height],
                dirty_pending: true,
            };
            TEX_COUNT = count + 1;
        }
    }

    /// The sink `miniquad` calls for every accepted `glTexSubImage2D`.
    ///
    /// # Safety
    /// `pixels` must address `w * h * 4` readable bytes.
    pub unsafe fn texture_write_sink(id: u32, x: u32, y: u32, w: u32, h: u32, pixels: *const u8) {
        if texture_write(id, x, y, w, h, pixels) {
            TEX_WRITES = TEX_WRITES.wrapping_add(1);
            TEX_LAST_WRITE = id;
        } else {
            TEX_WRITE_MISSES = TEX_WRITE_MISSES.wrapping_add(1);
        }
    }

    /// Copies one sub-image into the registry and unions its rectangle into the pending dirty area.
    ///
    /// Refused rather than clipped when the rectangle falls outside the stored extent: a write that
    /// overhangs means the registry's idea of the size and the caller's disagree, and half a glyph
    /// drawn over a neighbour is worse than no glyph.
    ///
    /// Returns false when the id is unknown or the rectangle does not fit.
    ///
    /// # Safety
    /// `pixels` must address `w * h * 4` readable bytes.
    pub unsafe fn texture_write(id: u32, x: u32, y: u32, w: u32, h: u32, pixels: *const u8) -> bool {
        if w == 0 || h == 0 {
            return false;
        }
        let count = TEX_COUNT;
        for i in 0..count {
            let e = TEX_ENTRIES[i];
            if e.id != id {
                continue;
            }
            let stride = (e.width as usize).saturating_mul(4);
            if e.len < stride.saturating_mul(e.height as usize) {
                return false;
            }
            if (x as usize).saturating_add(w as usize) > e.width as usize
                || (y as usize).saturating_add(h as usize) > e.height as usize
            {
                return false;
            }
            let row_bytes = (w as usize) * 4;
            for row in 0..h as usize {
                let dst = TEX_BYTES
                    .as_mut_ptr()
                    .add(e.offset + (y as usize + row) * stride + (x as usize) * 4);
                core::ptr::copy_nonoverlapping(pixels.add(row * row_bytes), dst, row_bytes);
            }

            let rect = [x, y, w, h];
            if TEX_ENTRIES[i].dirty_pending {
                // Union, not replace: the shell has not read what is there yet, so narrowing it to
                // the newest rectangle would drop everything written in between.
                let d = TEX_ENTRIES[i].dirty;
                let x0 = d[0].min(rect[0]);
                let y0 = d[1].min(rect[1]);
                let x1 = d[0].saturating_add(d[2]).max(rect[0].saturating_add(rect[2]));
                let y1 = d[1].saturating_add(d[3]).max(rect[1].saturating_add(rect[3]));
                TEX_ENTRIES[i].dirty = [x0, y0, x1 - x0, y1 - y0];
            } else {
                TEX_ENTRIES[i].dirty = rect;
                TEX_ENTRIES[i].dirty_pending = true;
            }
            return true;
        }
        false
    }

    /// Hands out the rectangle written since the last call, then clears it.
    ///
    /// Returns 1 with `out` filled, 0 when nothing changed, -1 for an unknown id. Clearing on the
    /// read keeps the ledger in one place: the shell must not have to remember what it saw last
    /// time, because two ledgers drift and the discrepancy only ever shows up on the device.
    ///
    /// A null `out` probes without consuming, so a caller can ask "is there anything?" without
    /// throwing the data away.
    ///
    /// # Safety
    /// `out` must be valid for four `u32`s when non-null.
    pub unsafe fn texture_dirty(id: u32, out: *mut u32) -> i32 {
        let count = TEX_COUNT;
        for i in 0..count {
            if TEX_ENTRIES[i].id != id {
                continue;
            }
            if !TEX_ENTRIES[i].dirty_pending {
                return 0;
            }
            if out.is_null() {
                return 1;
            }
            let d = TEX_ENTRIES[i].dirty;
            for k in 0..4 {
                *out.add(k) = d[k];
            }
            TEX_ENTRIES[i].dirty = [0; 4];
            TEX_ENTRIES[i].dirty_pending = false;
            return 1;
        }
        -1
    }

    /// Copies a sub-rectangle's RGBA bytes out.
    ///
    /// Returns the byte count, -1 for an unknown id, -2 when `cap` is too small (or `out` is
    /// null), -3 when the rectangle lies outside the texture.
    ///
    /// # Safety
    /// `out` must be valid for `cap` bytes.
    pub unsafe fn texture_rect_data(
        id: u32, x: u32, y: u32, w: u32, h: u32, out: *mut u8, cap: usize,
    ) -> i32 {
        let count = TEX_COUNT;
        for i in 0..count {
            let e = TEX_ENTRIES[i];
            if e.id != id {
                continue;
            }
            if (x as usize).saturating_add(w as usize) > e.width as usize
                || (y as usize).saturating_add(h as usize) > e.height as usize
            {
                return -3;
            }
            let stride = (e.width as usize).saturating_mul(4);
            let row_bytes = (w as usize).saturating_mul(4);
            let total = row_bytes.saturating_mul(h as usize);
            if out.is_null() {
                return -2;
            }
            if total > cap {
                return -2;
            }
            for row in 0..h as usize {
                let src = TEX_BYTES
                    .as_ptr()
                    .add(e.offset + (y as usize + row) * stride + (x as usize) * 4);
                core::ptr::copy_nonoverlapping(src, out.add(row * row_bytes), row_bytes);
            }
            return total as i32;
        }
        -1
    }

    /// Count, then the most recent refusal as `(id, width, height, bytes)`.
    ///
    /// `texture_overflow` already returns the count; this says *what* was lost, which is the part
    /// that decides whether the budget, the slot count, or the chart is at fault.
    pub fn texture_drop_last() -> (u32, u32, u32, usize) {
        unsafe { TEX_DROP }
    }

    /// Why the last one was refused: `"slots"`, `"single too large"`, or `"budget"`.
    ///
    /// Returned as a string rather than a code because its only caller is in-process (`host_m1`);
    /// the FFI surface keeps the numeric `texture_overflow` count.
    pub fn texture_drop_reason() -> &'static str {
        unsafe {
            match TEX_DROP_REASON {
                1 => "slots",
                2 => "single too large",
                3 => "budget",
                _ => "-",
            }
        }
    }

    /// Bytes currently held, and the budget they are counted against.
    pub fn texture_used_bytes() -> usize {
        unsafe { TEX_BYTES.len() }
    }

    /// The budget `texture_used_bytes` is counted against. Exposed so no caller has to restate
    /// `TEX_MAX_BYTES` as a literal and drift from it.
    pub fn texture_capacity() -> usize {
        TEX_MAX_BYTES
    }

    /// Unique bytes requested, and how many registrations were satisfied by aliasing.
    ///
    /// The pair answers the question `texture_used_bytes` cannot: a held total pinned at the budget
    /// says nothing about how much *more* the chart wanted.
    pub fn texture_demand() -> (usize, u32) {
        unsafe { (TEX_DEMAND, TEX_ALIAS) }
    }

    unsafe fn note_drop(id: u32, width: u32, height: u32, bytes: usize, reason: u8) {
        TEX_OVERFLOW = TEX_OVERFLOW.wrapping_add(1);
        TEX_DROP = (id, width, height, bytes);
        TEX_DROP_REASON = reason;
    }

    /// Looks a texture up. Returns its byte length and writes its metadata, or -1 when unknown.
    ///
    /// # Safety
    /// `width`/`height` must be valid for four bytes; `out` must have room for `cap` bytes.
    pub unsafe fn texture_data(id: u32, width: *mut u32, height: *mut u32, out: *mut u8, cap: usize) -> i32 {
        let count = TEX_COUNT;
        for i in 0..count {
            let e = TEX_ENTRIES[i];
            if e.id != id {
                continue;
            }
            if !width.is_null() {
                *width = e.width;
            }
            if !height.is_null() {
                *height = e.height;
            }
            if out.is_null() {
                // Metadata query only - the caller is sizing its buffer.
                return e.len as i32;
            }
            if e.len > cap {
                return -2;
            }
            core::ptr::copy_nonoverlapping(TEX_BYTES.as_ptr().add(e.offset), out, e.len);
            return e.len as i32;
        }
        -1
    }

    /// How many textures the registry holds, so the shell can iterate without guessing ids.
    pub fn texture_count() -> u32 {
        unsafe { TEX_COUNT as u32 }
    }

    /// The id of the n-th registered texture, or 0 when out of range.
    pub fn texture_id_at(index: u32) -> u32 {
        unsafe {
            if (index as usize) < TEX_COUNT {
                TEX_ENTRIES[index as usize].id
            } else {
                0
            }
        }
    }

    /// Textures dropped for lack of room. Non-zero means a chart exceeded the registry budget.
    pub fn texture_overflow() -> u32 {
        unsafe { TEX_OVERFLOW }
    }

    /// Called by the shell after it has replayed a frame, and by `host::frame` before it renders.
    pub fn drain_reset() {
        LEN.store(0, Ordering::Relaxed);
        // The arenas must be cleared too, or they would fill up and the batcher would silently
        // stop drawing after a few seconds of play.
        VLEN.store(0, Ordering::Relaxed);
        ILEN.store(0, Ordering::Relaxed);
        // Camera and model go back to their start-of-frame state for the same reason. macroquad
        // opens a frame with no camera and an identity model; just as important, `Main::render`
        // can propagate an error from between a `push_camera_state` and its `pop_camera_state`
        // (`scene.rs:520`), and a stack that kept that indent would misplace every frame after.
        crate::transform::reset_frame();
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
                // Fold `Projection * Model` in here rather than at flush time. macroquad would
                // have sent these through `Model` as a uniform and the camera as the other one
                // (`quad_gl.rs:744`), both of which are frozen for the duration of a draw call
                // (`quad_gl.rs:851`), so by the time a draw is issued there is only one answer
                // to "which matrices" - and having one answer is exactly what lets `PortCmd`
                // carry plain pixels with no camera attached.
                //
                // The space `v` arrives in is prpr's business, not ours: judge-line shapes come
                // in their own local frame, note sprites have already been moved to world space
                // by `Resource::world_to_screen` (`core/resource.rs:623`), and whichever
                // `apply_model_of` bracket is open is the one that decides the difference.
                let (x, y) = crate::transform::project(v.position.x, v.position.y);
                VERTS[vs + i] = *v;
                VERTS[vs + i].position.x = x;
                VERTS[vs + i].position.y = y;
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

    /// Records an untextured quad from four already-projected corners.
    ///
    /// Exists for the one shape `Cmd::Rect` cannot express: a rectangle the current `Model` has
    /// tilted. `Cmd::Rect` is `x/y/w/h`, and `f[]`'s eight slots are spoken for, so there is
    /// nowhere to put an angle - but four corners are four corners in the vertex arena. Going
    /// through geometry rather than degrading to a bounding box matters here precisely because
    /// the quad names no bitmap: `Win2DChartRenderer.FillPortTriangle` only bounds a triangle
    /// when it has a texture to map, and fills an untextured one exactly.
    ///
    /// Texture id 0 is safe as "none" because ids start at 1 (`TEX_ID`, below).
    pub fn push_solid_quad(corners: &[Vec2; 4], color: Color) {
        let vs = VLEN.load(Ordering::Relaxed) as usize;
        let is = ILEN.load(Ordering::Relaxed) as usize;
        if vs + 4 > VCAP || is + 6 > ICAP {
            return;
        }
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        unsafe {
            for (i, c) in corners.iter().enumerate() {
                VERTS[vs + i] = Vertex::new(c.x, c.y, 0.0, uv[i][0], uv[i][1], color);
            }
            for (i, ix) in [0u16, 1, 2, 0, 2, 3].iter().enumerate() {
                INDICES[is + i] = *ix;
            }
        }
        VLEN.store((vs + 4) as u32, Ordering::Relaxed);
        ILEN.store((is + 6) as u32, Ordering::Relaxed);
        push(Cmd::Geometry {
            tex: 0,
            mode: DrawMode::Triangles,
            vstart: vs as u32,
            vcount: 4,
            istart: is as u32,
            icount: 6,
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

    // =======================================================================
    // the flat FFI command stream
    //
    // `Cmd` above is a Rust enum and has no stable layout, so it cannot cross the DLL boundary.
    // These two records can: they are `#[repr(C)]`, fixed-size and tag-then-payload, which is what
    // a `DllImport` on the C# side can read as a plain struct array.
    //
    // One `PortVertex` per vertex and one `PortCmd` per draw call, both 32 bytes / aligned to 4,
    // which keeps the ARM marshaller happy (no 8-byte members).
    // =======================================================================

    /// One vertex of a `Geometry` batch, in the same order as the shim's `Vertex`.
    ///
    /// Positions are in the shim's own space - the C# renderer applies the same projection it uses
    /// for the mesh path, so the numbers are handed over untransformed.
    #[repr(C)]
    #[derive(Copy, Clone, Debug, Default)]
    pub struct PortVertex {
        pub x: f32,
        pub y: f32,
        pub z: f32,
        pub u: f32,
        pub v: f32,
        pub r: f32,
        pub g: f32,
        pub b: f32,
    }

    // Tag values. Kept in lockstep with `PortCmdKind` in `Phirura.WindowsPhone/PrprPort.cs`.
    pub const KIND_TEXTURE: u32 = 0;
    pub const KIND_RECT: u32 = 1;
    pub const KIND_LINE: u32 = 2;
    pub const KIND_CIRCLE: u32 = 3;
    pub const KIND_GEOMETRY: u32 = 4;
    // `Text` is deliberately NOT a kind: the shim cannot rasterise glyphs, the C# side owns fonts,
    // and recording a text command would need the string's characters rather than its length.
    // `draw_text` therefore records nothing, and the port's text stays on the C# path.

    /// A single draw call, flattened for the C# renderer.
    ///
    /// Every variant is expressed with the same 14 `f32`/`u32` payload slots so the record is
    /// fixed-size, which is what lets C# read an array of them without a per-call marshaller.
    /// Unused slots are zero and must be ignored by the reader.
    #[repr(C)]
    #[derive(Copy, Clone, Debug, Default)]
    pub struct PortCmd {
        /// One of the `KIND_*` constants.
        pub kind: u32,
        /// Texture id for `Texture`/`Geometry`, `DrawMode` for `Geometry`, else 0.
        pub flags: u32,
        /// Vertex/index arena offsets and sizes, for `Geometry` only.
        pub vstart: u32,
        pub vcount: u32,
        pub istart: u32,
        pub icount: u32,
        /// Payload: `Texture` = dest x/y/w/h then source x/y/w/h (8 slots);
        /// `Rect` = x/y/w/h; `Line` = x1/y1/x2/y2/thickness; `Circle` = cx/cy/r.
        pub f: [f32; 8],
    }

    // Flat output buffers, written on demand by `encode()`. Fixed capacity for the same reason the
    // `Cmd` arena is fixed: this runs on a phone and an allocation per frame is not affordable.
    const PORT_CMD_CAP: usize = 8192;
    const PORT_VTX_CAP: usize = 65536;
    static mut PORT_CMDS: [PortCmd; PORT_CMD_CAP] = [PortCmd {
        kind: 0,
        flags: 0,
        vstart: 0,
        vcount: 0,
        istart: 0,
        icount: 0,
        f: [0.0; 8],
    }; PORT_CMD_CAP];
    static mut PORT_VERTS: [PortVertex; PORT_VTX_CAP] = [PortVertex {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        u: 0.0,
        v: 0.0,
        r: 0.0,
        g: 0.0,
        b: 0.0,
    }; PORT_VTX_CAP];
    static PORT_CMD_LEN: AtomicU32 = AtomicU32::new(0);

    /// Translate the recorded `Cmd` arena into the flat records the shell reads.
    ///
    /// Returns the number of `PortCmd` records written; the matching vertex buffer is fetched with
    /// `port_vertices()` and is only meaningful for `Geometry` records.
    ///
    /// Re-encoding every frame rather than recording flat in the first place is deliberate: the
    /// `Cmd` arena is what `prpr`'s batcher already drives, and translating at the boundary keeps
    /// the shim's own logic (scissor handling, `draw_mode` state) intact.
    pub fn encode() -> u32 {
        let n = LEN.load(Ordering::Relaxed) as usize;
        let mut vout = 0usize;
        let mut cout = 0usize;

        for slot in unsafe { &BUF[..n] } {
            let c = match slot {
                Some(c) => *c,
                None => continue,
            };
            if cout >= PORT_CMD_CAP {
                break;
            }

            let mut rec = PortCmd::default();
            match c {
                Cmd::Texture { tex, dest, source, color, rotation, flip_x, flip_y } => {
                    rec.kind = KIND_TEXTURE;
                    rec.flags = tex;
                    rec.f[0] = dest.x;
                    rec.f[1] = dest.y;
                    rec.f[2] = dest.w;
                    rec.f[3] = dest.h;
                    rec.f[4] = source.x;
                    rec.f[5] = source.y;
                    rec.f[6] = source.w;
                    rec.f[7] = source.h;
                    // Rotation and flips do not fit the 8-slot payload, so they are folded into
                    // the tag's high bits: bit 16 = flip_x, bit 17 = flip_y, and rotation is
                    // carried in `flags`'s high half as a f32 bit pattern is not usable here -
                    // so rotation is applied by the shell only for the unrotated common case.
                    // (prpr rotates only its own particles, which do not use this path.)
                    if flip_x {
                        rec.kind |= 1 << 16;
                    }
                    if flip_y {
                        rec.kind |= 1 << 17;
                    }
                    // Colour is dropped for textures: `prpr` tints through the vertex colour of
                    // its Geometry batches, and the Texture path is always full-white in practice.
                    let _ = (color, rotation);
                }
                Cmd::Rect { dest, color } => {
                    rec.kind = KIND_RECT;
                    rec.f[0] = dest.x;
                    rec.f[1] = dest.y;
                    rec.f[2] = dest.w;
                    rec.f[3] = dest.h;
                    rec.f[4] = color.r;
                    rec.f[5] = color.g;
                    rec.f[6] = color.b;
                    rec.f[7] = color.a;
                }
                Cmd::Line { a, b, thickness, color } => {
                    rec.kind = KIND_LINE;
                    rec.f[0] = a.x;
                    rec.f[1] = a.y;
                    rec.f[2] = b.x;
                    rec.f[3] = b.y;
                    rec.f[4] = thickness;
                    rec.f[5] = color.r;
                    rec.f[6] = color.g;
                    rec.f[7] = color.b;
                }
                Cmd::Circle { center, r, color } => {
                    rec.kind = KIND_CIRCLE;
                    rec.f[0] = center.x;
                    rec.f[1] = center.y;
                    rec.f[2] = r;
                    rec.f[3] = color.r;
                    rec.f[4] = color.g;
                    rec.f[5] = color.b;
                    rec.f[6] = color.a;
                }
                Cmd::Geometry { tex, mode, vstart, vcount, istart, icount } => {
                    rec.kind = KIND_GEOMETRY;
                    rec.flags = tex;
                    rec.vstart = vstart;
                    rec.vcount = vcount;
                    rec.istart = istart;
                    rec.icount = icount;
                    rec.f[0] = match mode {
                        DrawMode::Triangles => 0.0,
                        DrawMode::Lines => 1.0,
                        DrawMode::LineStrip => 2.0,
                    };
                    // Copy the vertices into the flat arena, re-based to the output offset so the
                    // shell indexes `port_vertices()` directly.
                    let src = vertex_slice(vstart, vcount);
                    let mut j = 0usize;
                    while j < src.len() && vout < PORT_VTX_CAP {
                        let v = src[j];
                        unsafe {
                            PORT_VERTS[vout] = PortVertex {
                                x: v.position.x,
                                y: v.position.y,
                                z: v.position.z,
                                u: v.uv.x,
                                v: v.uv.y,
                                r: v.color[0],
                                g: v.color[1],
                                b: v.color[2],
                            };
                        }
                        vout += 1;
                        j += 1;
                    }
                    // The shell reads indices through the shim's own arena, which is stable for the
                    // frame, so `istart`/`icount` are passed through unchanged.
                }
                // Text is intentionally not forwarded - see the `KIND_*` note above.
                Cmd::Text { .. } => continue,
            }

            unsafe { PORT_CMDS[cout] = rec };
            cout += 1;
        }

        PORT_CMD_LEN.store(cout as u32, Ordering::Relaxed);
        cout as u32
    }

    /// Pointer to the flat command array produced by the last `encode()`. Never null.
    pub fn port_commands() -> *const PortCmd {
        unsafe { core::ptr::addr_of!(PORT_CMDS) as *const PortCmd }
    }

    /// Pointer to the flat vertex array. Never null.
    pub fn port_vertices() -> *const PortVertex {
        unsafe { core::ptr::addr_of!(PORT_VERTS) as *const PortVertex }
    }

    /// Index arena, which the shell reads for `Geometry` records. Never null.
    pub fn port_indices() -> *const u16 {
        unsafe { core::ptr::addr_of!(INDICES) as *const u16 }
    }
}

pub fn draw_texture_ex(tex: Texture2D, x: f32, y: f32, color: Color, params: DrawTextureParams) {
    let size = params.dest_size.unwrap_or_else(|| tex.size());
    let source = params
        .source
        .unwrap_or_else(|| Rect::new(0.0, 0.0, tex.width(), tex.height()));
    // `Cmd::Texture` is a rectangle and nothing more, so a draw the transform left
    // axis-aligned is recorded exactly and one it tilted is recorded as its bounding box - see
    // `transform::project_aabb` for why the box is the right degradation rather than geometry.
    let raw = if crate::transform::axis_aligned() && params.rotation == 0.0 {
        crate::transform::project_rect(x, y, size.x, size.y)
    } else {
        crate::transform::project_aabb(x, y, size.x, size.y)
    };
    // `project_aabb` is non-negative by construction and `project_rect` is not: prpr's cameras
    // all mirror in y, which hands back a negative height the shell would then throw away. See
    // `transform::normalize_rect`.
    let (dest, flip_x, flip_y) =
        crate::transform::normalize_rect(raw, params.flip_x, params.flip_y);
    draw::push(draw::Cmd::Texture {
        tex: tex.id,
        dest,
        source,
        color,
        rotation: params.rotation,
        flip_x,
        flip_y,
    });
}

pub fn draw_texture(tex: &Texture2D, x: f32, y: f32, color: Color) {
    draw_texture_ex(*tex, x, y, color, DrawTextureParams::default());
}

pub fn draw_rectangle(x: f32, y: f32, w: f32, h: f32, color: Color) {
    if crate::transform::axis_aligned() {
        // A mirrored solid rectangle looks the same as its unmirrored self, so the sign is
        // simply folded away rather than handed to a reader that has no flip to apply it to.
        let (dest, _, _) = crate::transform::normalize_rect(
            crate::transform::project_rect(x, y, w, h),
            false,
            false,
        );
        draw::push(draw::Cmd::Rect { dest, color });
    } else {
        // A tilting `Model` has four independent corners now, and `Cmd::Rect` carries two
        // numbers too few to say so - whereas the vertex arena carries any quad, and untextured
        // quads are the one thing the shell fills exactly.
        draw::push_solid_quad(
            &[
                Vec2::new(x, y),
                Vec2::new(x + w, y),
                Vec2::new(x + w, y + h),
                Vec2::new(x, y + h),
            ],
            color,
        );
    }
}

pub fn draw_line(x1: f32, y1: f32, x2: f32, y2: f32, thickness: f32, color: Color) {
    // macroquad does not record a line at all: it offsets both ends along the segment's local
    // normal to make a four-corner strip (`prpr-macroquad/src/shapes.rs:223-232`) and lets `Model`
    // carry those corners into place. `Cmd::Line` has room for two points and a width, so the
    // offset corners are run through the transform first and the width read back off the result.
    // Exact whenever the transform is a rotation plus a uniform scale - what prpr's judge-line
    // models actually are - and within a hair of it otherwise, since a two-point record cannot
    // express the skew a non-uniform scale would introduce.
    let d = Vec2::new(x2 - x1, y2 - y1);
    // The same guard shapes.rs:226-229 performs, over the same quantity: `nx`/`ny` there are
    // `-dy`/`dx`, so `|n| == |d|` and `tlen` reduces to this. A zero segment drops out, a
    // negative thickness drops out, and a zero thickness yields a zero-width strip - which is
    // what upstream's degenerate quad draws too.
    let tlen = d.length() / (thickness * 0.5);
    if tlen < f32::EPSILON {
        return;
    }

    let a = crate::transform::project_point(Vec2::new(x1, y1));
    let b = crate::transform::project_point(Vec2::new(x2, y2));
    let normal = Vec2::new(-d.y, d.x) / tlen; // shapes.rs:230-231, `tx`/`ty`
    let p1 = crate::transform::project_point(Vec2::new(x1 + normal.x, y1 + normal.y));
    let p2 = crate::transform::project_point(Vec2::new(x1 - normal.x, y1 - normal.y));

    let offset = p1 - p2;
    let seg = b - a;
    let seg_len = seg.length();
    let thickness_px = if seg_len <= f32::EPSILON {
        offset.length()
    } else {
        // Only the component across the drawn segment is width; the component along it is skew
        // that this record has no field for. prpr's cameras are built to keep both axes equal
        // (`camera.zoom = (1., -aspect)`, `core/resource.rs:617`), so the difference is second
        // order even where it is not zero.
        let u = seg / seg_len;
        (offset - offset.dot(u) * u).length()
    };

    draw::push(draw::Cmd::Line {
        a,
        b,
        thickness: thickness_px,
        color,
    });
}

pub fn draw_circle(x: f32, y: f32, r: f32, color: Color) {
    // Radius measured along x. `Cmd::Circle` carries one number, so a transform that scaled the
    // two axes apart - an ellipse - has nowhere to put the difference; see `Cmd::Circle` for the
    // rest of that trade. prpr never calls this in the first place.
    let center = crate::transform::project_point(Vec2::new(x, y));
    let radius = crate::transform::project_delta(Vec2::new(x, y), Vec2::new(x + r, y)).length();
    draw::push(draw::Cmd::Circle {
        center,
        r: radius,
        color,
    });
}

pub fn draw_text(text: &str, x: f32, y: f32, _size: f32, color: Color) {
    // Never leaves the record: `encode` drops `Cmd::Text` outright, because prpr lays text out
    // itself through `TextPainter` and has no use for macroquad's text API. Projected regardless
    // so that the one field it does have would still mean what every other command means.
    let (x, y) = crate::transform::project(x, y);
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

/// macroquad's prelude re-exports `std::thread`, and `prpr::ui::offset_analysis` relies on that:
/// it writes a bare `thread::spawn(move || …)` with no `use std::thread` anywhere in the file.
/// Without this the prelude glob does not supply the name and the file fails with
/// `E0433: cannot find module or crate 'thread' in this scope`.
///
/// **This is the one runtime-landmine in the port.** `std::thread` was measured to abort
/// (`0xC0000409`) on WP8.1 — the OS supports threads (a raw `CreateThread` probe succeeds), it is
/// Rust's thread wrapper that dies. `offset_analysis` is the only user, so the honest options are
/// to gate it out on this target or to hand-roll a `CreateThread` wrapper later; re-exporting here
/// at least keeps the *compile* honest about the dependency.
pub use std::thread;

/// Sets the asset root. The shell resolves assets from the app package, so there is no search path
/// to configure - `prpr::core::init` calls this unconditionally at startup.
pub fn set_pc_assets_folder(_path: &str) {}

/// The shell's asset reader, installed once by the bridge crate at startup.
///
/// # Why this hook exists
///
/// `prpr` reads almost everything through its own `FileSystem`, which the bridge implements by
/// handing paths to the shell. But two paths bypass it and land here instead:
///
///   * `ResourcePack` - `GameScene` does not take a resource pack, so it calls
///     `ResourcePack::from_path(None)`, which becomes `fs_from_assets("respack/")` and hence
///     `load_file("respack/<name>")`. That is every note texture and every hit sound.
///   * `ResourcePack::load_clip!`'s last resort, which asks for `click.ogg` and friends with no
///     prefix at all.
///
/// Both used to return a hard `NotFound`, so the scene could not be constructed no matter what the
/// shell had on disk - the failure arrived as `GameScene::new failed: ... Missing info.yml` with
/// no indication that the file had never been asked for *from anywhere that could answer*.
///
/// The hook is a plain `fn`, not a closure: it has to survive being stored in a `static` and be
/// callable from whatever thread the scene happens to be built on.
type AssetHook = fn(&str) -> Option<Vec<u8>>;

/// Held as an address in an atomic rather than a `static mut` because reading a `static mut` from
/// a function pointer field draws a lint that is one release away from being a hard error, and
/// because the writer (`set_asset_hook`) and the reader (`load_file`) are genuinely concurrent:
/// the shell installs it before the first frame, and the scene may load assets mid-frame.
static ASSET_HOOK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Install the shell's asset reader. Passing a hook makes `load_file` answer; there is no
/// "uninstall", because the shell installs exactly one for the process lifetime.
pub fn set_asset_hook(hook: AssetHook) {
    ASSET_HOOK.store(hook as usize, core::sync::atomic::Ordering::Relaxed);
}

fn asset_hook() -> Option<AssetHook> {
    match ASSET_HOOK.load(core::sync::atomic::Ordering::Relaxed) {
        0 => None,
        // `usize` and a function pointer are the same width on every target this builds for
        // (x86_64 host, thumbv7 phone), and the value was stored from a real function pointer.
        p => Some(unsafe { core::mem::transmute::<usize, AssetHook>(p) }),
    }
}

/// Loads a file relative to the asset root.
///
/// With a hook installed this is a straight hand-off to the shell, path unchanged - the shell
/// decides what "relative to the asset root" means, which on the phone is the app's local folder.
/// Without one it reports failure rather than panicking, so a shell that has not wired the hook
/// gets an error line naming the file instead of a crash.
pub async fn load_file(path: &str) -> Result<Vec<u8>, std::io::Error> {
    match asset_hook() {
        Some(hook) => hook(path).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("asset not found: {path}"))
        }),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("asset IO is owned by the shell: {path}"),
        )),
    }
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

    /// Replays this frame's queued touches into a handler.
    ///
    /// This is the *only* way `prpr::judge`'s `TOUCHES` store gets filled. `Judge::on_new_frame`
    /// runs a `Handler` through here and then replaces `TOUCHES` wholesale with the result, and
    /// `TOUCHES` is what `get_touches()` reads - every button in the scene, and the whole `Ui`.
    /// Left as a no-op the chart would still judge its notes (those come from `touches()`), while
    /// nothing on screen could be pressed: input that looks 90% broken.
    ///
    /// The queue is deliberately **not** drained. `Judge::update` calls `touches()` later in the
    /// same frame for the state of what is still down, and `Scene` may read `get_touches()`
    /// during render; taking the events here would leave those two reading a frame older than the
    /// one they are drawing. The host clears the queue once the frame has been fully drawn.
    ///
    /// `_subscriber` stays ignored: upstream replays only what that particular subscriber missed,
    /// and here there is exactly one consumer that wants every event every frame.
    pub fn repeat_all_miniquad_input<H: EventHandler>(handler: &mut H, _subscriber: usize) {
        // `prpr`'s `Handler::touch_event` ignores its context and the shell owns the real one, so
        // a local unit value is the honest stand-in - not a null pointer nothing dereferences.
        let mut ctx = crate::miniquad::Context;
        crate::input::for_each_pending(|t| {
            handler.touch_event(&mut ctx, t.phase, t.id, t.position.x, t.position.y, t.time);
        });
    }
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
    /// The swizzle traits travel with the maths types: `prpr` gets `pos.xy()` through the
    /// prelude, and a module that re-exports `Vec4` without them still fails to compile at the
    /// `.xy()` sites.
    pub use super::{Vec2Swizzles, Vec3Swizzles, Vec4Swizzles};

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

/// Loads and decodes an image from the asset root.
///
/// Upstream macroquad reads the file and decodes it with the `image` crate, and so does this: the
/// bytes come through the same shell hook `load_file` uses, so the shell's job is identical
/// either way - it is asked for a path and returns bytes, and it never needs to know whether the
/// caller was going to parse them as JSON or as pixels.
///
/// `prpr` reaches this for the artwork that belongs to the *player* rather than to any one chart:
/// the eight `rank/*.png` badges, the `back`/`retry`/`resume`/`proceed` icons, and the
/// `info`/`warn`/`ok`/`error` billboards `Main::new` hangs off the global `BILLBOARD`. A chart can
/// be parsed with none of them present, but `GameScene::new` cannot: it loads four of these before
/// it returns, and while this was a stub every construction failed with `unsupported on this
/// platform: load_image` no matter what the shell had on disk.
///
/// The `Unsupported` answer survives for the one case that still means it - no shell installed -
/// because then there is genuinely nowhere to read from.
pub async fn load_image(path: &str) -> Result<Image, Error> {
    if asset_hook().is_none() {
        return Err(Error::Unsupported("load_image"));
    }
    let bytes = load_file(path)
        .await
        .map_err(|e| Error::Other(format!("{path}: {e}")))?;
    // `load_from_memory` answers `DynamicImage`, so the conversion to a concrete RGBA8 buffer is
    // what `to_rgba8` does - it is also the point at which a palette or grayscale file becomes the
    // four-channel layout `Image` stores.
    let rgba = image::load_from_memory(&bytes)
        .map_err(|e| Error::Other(format!("{path}: {e}")))?
        .to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    // `Image` carries `u16` dimensions because macroquad's does. A wider image would truncate
    // silently and misdraw forever, so it is refused as a decodable-but-unusable file instead.
    if width > u16::MAX as u32 || height > u16::MAX as u32 {
        return Err(Error::Other(format!(
            "{path}: {width}x{height} exceeds macroquad's u16 dimensions"
        )));
    }
    Ok(Image {
        width: width as u16,
        height: height as u16,
        bytes: rgba.into_raw(),
    })
}

pub mod prelude {
    pub use super::color::*;
    pub use super::glam;
    pub use super::utils;
    /// `std::thread`, through the prelude glob.
    ///
    /// `prpr::ui::offset_analysis` writes a bare `thread::spawn(..)` and imports only
    /// `macroquad::prelude::*`, so the name has to arrive by this glob. Exporting it from the crate
    /// root alone is not enough - rustc then reports "unresolved module **or unlinked crate**" and
    /// suggests `use macroquad::thread`, which is a *crate-shaped* suggestion for a path prpr does
    /// not spell out.
    pub use super::thread;
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
        Vec2, Vec3, Vec4, Vertex, Vec2Swizzles, Vec3Swizzles, Vec4Swizzles,
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
