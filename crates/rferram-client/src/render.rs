//! `wgpu` (`30.0.1`) Zero-Copy Texture Render Pipeline, Layout Calculators & WGSL Video Surface Shader.
//!
//! Provides cross-platform texture plane layout calculations (`NV12`, `YUV420P`, `YUV444P`, `Rgba8`,
//! `Bgra8`), `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (`256`-byte) row stride padding, buffer bounds
//! verification, and `VIDEO_SURFACE_SHADER_WGSL` validation for Metal, Vulkan, and DirectX 12.

use std::borrow::Cow;

use anyhow::{Result, bail, ensure};
use rferram_protocol::PixelFormat;

/// Built-in WGSL Shader for Zero-Copy BGRA8 / NV12 Frame Presentation on Vulkan, Metal, and DX12.
pub const VIDEO_SURFACE_SHADER_WGSL: &str = r#"
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32((in_vertex_index << 1u) & 2u);
    let y = f32(in_vertex_index & 2u);
    out.clip_position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.tex_coords = vec2<f32>(x, y);
    return out;
}

@group(0) @binding(0)
var frame_tex: texture_2d<f32>;
@group(0) @binding(1)
var frame_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(frame_tex, frame_sampler, in.tex_coords);
}
"#;

/// Mandatory `wgpu` row pitch alignment in bytes (`256`) for `Queue::write_texture` and buffer copies.
pub const WGPU_BYTES_PER_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// Maximum supported 2D texture dimension (`16384` texels) across Vulkan, Metal, and DirectX 12.
pub const MAX_TEXTURE_DIMENSION_2D: u32 = 16_384;

/// Decoded surface frame ready for WGPU Metal/Vulkan/DX12 texture binding and presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrameSurface {
    pub frame_index: u64,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
    pub timestamp_us: u64,
}

impl DecodedFrameSurface {
    /// Calculates the multi-planar or single-planar [`TextureSurfaceLayout`] for this surface's dimensions and format.
    pub fn calculate_layout(&self) -> Result<TextureSurfaceLayout> {
        WgpuRenderPipeline::calculate_surface_layout(self.width, self.height, self.format)
    }

    /// Validates dimensions and verifies that `self.data` contains enough bytes for all planes.
    pub fn validate(&self) -> Result<TextureSurfaceLayout> {
        WgpuRenderPipeline::validate_frame_surface(self)
    }
}

/// Per-plane layout metrics and `wgpu` upload parameters for single-planar or multi-planar video frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexturePlaneLayout {
    /// Zero-based plane index (`0` for `Bgra8`/`Rgba8` or `Y` luma; `1` for `UV` or `U` chroma; `2` for `V` chroma).
    pub plane_index: u32,
    /// Width of this plane in texels.
    pub width: u32,
    /// Height of this plane in texels.
    pub height: u32,
    /// Bytes per texel in this plane (`4` for `Bgra8`/`Rgba8`, `1` for `Y`/`U`/`V`, `2` for `NV12` `UV`).
    pub bytes_per_pixel: u32,
    /// Tightly packed row stride in bytes (`width * bytes_per_pixel`).
    pub unpadded_bytes_per_row: u32,
    /// `wgpu`-aligned row stride in bytes (rounded up to a multiple of [`WGPU_BYTES_PER_ROW_ALIGNMENT`]).
    pub aligned_bytes_per_row: u32,
    /// Number of texel rows in this plane (`height`).
    pub rows_per_image: u32,
    /// Byte offset of this plane inside a tightly-packed contiguous frame buffer.
    pub byte_offset: usize,
    /// Tightly packed byte size of this plane (`unpadded_bytes_per_row * height`).
    pub unpadded_byte_size: usize,
    /// `wgpu`-aligned byte size of this plane (`aligned_bytes_per_row * height`).
    pub aligned_byte_size: usize,
    /// Target `wgpu::TextureFormat` for uploading and sampling this plane.
    pub wgpu_format: wgpu::TextureFormat,
}

impl TexturePlaneLayout {
    /// Returns the `wgpu::Extent3d` for creating a 2D texture for this plane.
    pub fn extent_3d(&self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.width,
            height: self.height,
            depth_or_array_layers: 1,
        }
    }

    /// Returns the `wgpu::TexelCopyBufferLayout` for uploading this plane to a `wgpu::Texture`.
    pub fn texel_copy_buffer_layout(
        &self,
        use_aligned_stride: bool,
    ) -> wgpu::TexelCopyBufferLayout {
        let bytes_per_row = if use_aligned_stride {
            self.aligned_bytes_per_row
        } else {
            self.unpadded_bytes_per_row
        };
        wgpu::TexelCopyBufferLayout {
            offset: if use_aligned_stride {
                0
            } else {
                self.byte_offset as u64
            },
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(self.rows_per_image),
        }
    }
}

/// Complete texture layout description for a decoded video frame across all constituent planes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureSurfaceLayout {
    /// Full luma/surface width in pixels.
    pub width: u32,
    /// Full luma/surface height in pixels.
    pub height: u32,
    /// Wire pixel format of the surface.
    pub pixel_format: PixelFormat,
    /// Primary `wgpu::TextureFormat` representing the surface.
    pub primary_wgpu_format: wgpu::TextureFormat,
    /// Ordered plane descriptors (`1` plane for `Bgra8`/`Rgba8`, `2` for `NV12`, `3` for `YUV420P`/`YUV444P`).
    pub planes: Vec<TexturePlaneLayout>,
    /// Total tightly packed byte size across all planes.
    pub total_unpadded_bytes: usize,
    /// Total byte size across all planes when each row is padded to [`WGPU_BYTES_PER_ROW_ALIGNMENT`].
    pub total_aligned_bytes: usize,
}

impl TextureSurfaceLayout {
    /// Number of texture planes required for this surface format.
    pub fn plane_count(&self) -> usize {
        self.planes.len()
    }

    /// Tightly packed row stride of plane 0 in bytes.
    pub fn primary_unpadded_stride(&self) -> u32 {
        self.planes
            .first()
            .map_or(0, |plane| plane.unpadded_bytes_per_row)
    }

    /// `wgpu`-aligned row stride of plane 0 in bytes.
    pub fn primary_aligned_stride(&self) -> u32 {
        self.planes
            .first()
            .map_or(0, |plane| plane.aligned_bytes_per_row)
    }

    /// Returns the full-surface `wgpu::Extent3d`.
    pub fn extent_3d(&self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.width,
            height: self.height,
            depth_or_array_layers: 1,
        }
    }

    /// Validates that a caller-supplied row stride for `plane_index` is at least the minimum unpadded row stride.
    pub fn validate_stride(&self, plane_index: usize, stride: u32) -> Result<()> {
        let Some(plane) = self.planes.get(plane_index) else {
            bail!(
                "Plane index {} out of bounds for {:?} surface ({} planes)",
                plane_index,
                self.pixel_format,
                self.planes.len()
            );
        };
        if stride < plane.unpadded_bytes_per_row {
            bail!(
                "Plane {} stride ({}) is smaller than minimum required row stride ({}) for {:?} {}x{}",
                plane_index,
                stride,
                plane.unpadded_bytes_per_row,
                self.pixel_format,
                self.width,
                self.height
            );
        }
        Ok(())
    }

    /// Validates that a frame buffer has sufficient length to hold all tightly packed planes.
    pub fn validate_buffer_len(&self, buffer_len: usize) -> Result<()> {
        if buffer_len < self.total_unpadded_bytes {
            bail!(
                "Frame buffer length ({}) is smaller than minimum required byte size ({}) for {:?} {}x{}",
                buffer_len,
                self.total_unpadded_bytes,
                self.pixel_format,
                self.width,
                self.height
            );
        }
        Ok(())
    }
}

/// `wgpu` (`30.0.1`) Zero-Copy Texture Render Pipeline descriptor and layout calculator.
#[derive(Debug, Clone)]
pub struct WgpuRenderPipeline {
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub layout: TextureSurfaceLayout,
    pub vsync_enabled: bool,
}

impl WgpuRenderPipeline {
    /// Creates a new [`WgpuRenderPipeline`] after validating [`VIDEO_SURFACE_SHADER_WGSL`] and surface dimensions.
    pub fn new(width: u32, height: u32, pixel_format: PixelFormat) -> Result<Self> {
        let _ = Self::validate_default_shader()?;
        let layout = Self::calculate_surface_layout(width, height, pixel_format)?;
        Ok(Self {
            width,
            height,
            pixel_format,
            layout,
            vsync_enabled: false,
        })
    }

    /// Recomputes the pipeline's texture layout when the remote monitor resolution or pixel format changes.
    pub fn reconfigure(
        &mut self,
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    ) -> Result<&TextureSurfaceLayout> {
        let layout = Self::calculate_surface_layout(width, height, pixel_format)?;
        self.width = width;
        self.height = height;
        self.pixel_format = pixel_format;
        self.layout = layout;
        Ok(&self.layout)
    }

    /// Aligns an unpadded row byte count up to the nearest multiple of [`WGPU_BYTES_PER_ROW_ALIGNMENT`] (`256`).
    pub const fn align_bytes_per_row(unpadded_bytes_per_row: u32) -> u32 {
        let align = WGPU_BYTES_PER_ROW_ALIGNMENT;
        let rem = unpadded_bytes_per_row % align;
        if rem == 0 {
            unpadded_bytes_per_row
        } else {
            unpadded_bytes_per_row.saturating_add(align - rem)
        }
    }

    /// Validates that texture dimensions are non-zero and within GPU 2D texture limits.
    pub fn validate_dimensions(width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            bail!(
                "Invalid texture surface dimensions: {}x{} (width and height must be non-zero)",
                width,
                height
            );
        }
        if width > MAX_TEXTURE_DIMENSION_2D || height > MAX_TEXTURE_DIMENSION_2D {
            bail!(
                "Texture surface dimensions {}x{} exceed maximum 2D limit ({})",
                width,
                height,
                MAX_TEXTURE_DIMENSION_2D
            );
        }
        Ok(())
    }

    /// Returns the minimum tightly-packed row stride in bytes for plane 0 of the given width and [`PixelFormat`].
    pub fn minimum_row_stride(width: u32, pixel_format: PixelFormat) -> Result<u32> {
        ensure!(
            width > 0 && width <= MAX_TEXTURE_DIMENSION_2D,
            "Invalid texture width: {width}"
        );
        let stride = match pixel_format {
            PixelFormat::Bgra8 | PixelFormat::Rgba8 => width
                .checked_mul(4)
                .ok_or_else(|| anyhow::anyhow!("Row stride overflow for width {width}"))?,
            PixelFormat::NV12 | PixelFormat::YUV420P | PixelFormat::YUV444P => width,
        };
        Ok(stride)
    }

    /// Returns the `wgpu` 256-byte-aligned row stride in bytes for plane 0 of the given width and [`PixelFormat`].
    pub fn aligned_row_stride(width: u32, pixel_format: PixelFormat) -> Result<u32> {
        let unpadded = Self::minimum_row_stride(width, pixel_format)?;
        Ok(Self::align_bytes_per_row(unpadded))
    }

    /// Calculates the complete [`TextureSurfaceLayout`] across all planes for `NV12`, `YUV420P`, `YUV444P`, `Rgba8`, or `Bgra8`.
    pub fn calculate_surface_layout(
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    ) -> Result<TextureSurfaceLayout> {
        Self::validate_dimensions(width, height)?;

        let build_plane = |plane_index: u32,
                           plane_w: u32,
                           plane_h: u32,
                           bytes_per_pixel: u32,
                           byte_offset: usize,
                           wgpu_format: wgpu::TextureFormat|
         -> Result<TexturePlaneLayout> {
            let unpadded_bytes_per_row = plane_w
                .checked_mul(bytes_per_pixel)
                .ok_or_else(|| anyhow::anyhow!("Plane {plane_index} row stride overflow"))?;
            let aligned_bytes_per_row = Self::align_bytes_per_row(unpadded_bytes_per_row);
            let unpadded_byte_size = (unpadded_bytes_per_row as usize)
                .checked_mul(plane_h as usize)
                .ok_or_else(|| anyhow::anyhow!("Plane {plane_index} unpadded size overflow"))?;
            let aligned_byte_size = (aligned_bytes_per_row as usize)
                .checked_mul(plane_h as usize)
                .ok_or_else(|| anyhow::anyhow!("Plane {plane_index} aligned size overflow"))?;

            Ok(TexturePlaneLayout {
                plane_index,
                width: plane_w,
                height: plane_h,
                bytes_per_pixel,
                unpadded_bytes_per_row,
                aligned_bytes_per_row,
                rows_per_image: plane_h,
                byte_offset,
                unpadded_byte_size,
                aligned_byte_size,
                wgpu_format,
            })
        };

        let (primary_wgpu_format, planes) = match pixel_format {
            PixelFormat::Bgra8 => {
                let p0 = build_plane(0, width, height, 4, 0, wgpu::TextureFormat::Bgra8UnormSrgb)?;
                (wgpu::TextureFormat::Bgra8UnormSrgb, vec![p0])
            }
            PixelFormat::Rgba8 => {
                let p0 = build_plane(0, width, height, 4, 0, wgpu::TextureFormat::Rgba8UnormSrgb)?;
                (wgpu::TextureFormat::Rgba8UnormSrgb, vec![p0])
            }
            PixelFormat::NV12 => {
                let uv_w = width.div_ceil(2);
                let uv_h = height.div_ceil(2);
                let y_plane = build_plane(0, width, height, 1, 0, wgpu::TextureFormat::R8Unorm)?;
                let uv_plane = build_plane(
                    1,
                    uv_w,
                    uv_h,
                    2,
                    y_plane.unpadded_byte_size,
                    wgpu::TextureFormat::Rg8Unorm,
                )?;
                (wgpu::TextureFormat::NV12, vec![y_plane, uv_plane])
            }
            PixelFormat::YUV420P => {
                let chroma_w = width.div_ceil(2);
                let chroma_h = height.div_ceil(2);
                let y_plane = build_plane(0, width, height, 1, 0, wgpu::TextureFormat::R8Unorm)?;
                let u_plane = build_plane(
                    1,
                    chroma_w,
                    chroma_h,
                    1,
                    y_plane.unpadded_byte_size,
                    wgpu::TextureFormat::R8Unorm,
                )?;
                let v_offset = y_plane
                    .unpadded_byte_size
                    .checked_add(u_plane.unpadded_byte_size)
                    .ok_or_else(|| anyhow::anyhow!("YUV420P V-plane offset overflow"))?;
                let v_plane = build_plane(
                    2,
                    chroma_w,
                    chroma_h,
                    1,
                    v_offset,
                    wgpu::TextureFormat::R8Unorm,
                )?;
                (
                    wgpu::TextureFormat::R8Unorm,
                    vec![y_plane, u_plane, v_plane],
                )
            }
            PixelFormat::YUV444P => {
                let y_plane = build_plane(0, width, height, 1, 0, wgpu::TextureFormat::R8Unorm)?;
                let u_plane = build_plane(
                    1,
                    width,
                    height,
                    1,
                    y_plane.unpadded_byte_size,
                    wgpu::TextureFormat::R8Unorm,
                )?;
                let v_offset = y_plane
                    .unpadded_byte_size
                    .checked_add(u_plane.unpadded_byte_size)
                    .ok_or_else(|| anyhow::anyhow!("YUV444P V-plane offset overflow"))?;
                let v_plane =
                    build_plane(2, width, height, 1, v_offset, wgpu::TextureFormat::R8Unorm)?;
                (
                    wgpu::TextureFormat::R8Unorm,
                    vec![y_plane, u_plane, v_plane],
                )
            }
        };

        let mut total_unpadded_bytes = 0usize;
        let mut total_aligned_bytes = 0usize;
        for plane in &planes {
            total_unpadded_bytes = total_unpadded_bytes
                .checked_add(plane.unpadded_byte_size)
                .ok_or_else(|| anyhow::anyhow!("Total unpadded byte size overflow"))?;
            total_aligned_bytes = total_aligned_bytes
                .checked_add(plane.aligned_byte_size)
                .ok_or_else(|| anyhow::anyhow!("Total aligned byte size overflow"))?;
        }

        Ok(TextureSurfaceLayout {
            width,
            height,
            pixel_format,
            primary_wgpu_format,
            planes,
            total_unpadded_bytes,
            total_aligned_bytes,
        })
    }

    /// Returns the total tightly-packed frame byte size across all planes for the given dimensions and format.
    pub fn expected_frame_bytes(
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    ) -> Result<usize> {
        let layout = Self::calculate_surface_layout(width, height, pixel_format)?;
        Ok(layout.total_unpadded_bytes)
    }

    /// Validates a [`DecodedFrameSurface`]'s dimensions, plane layout, and payload byte size.
    pub fn validate_frame_surface(surface: &DecodedFrameSurface) -> Result<TextureSurfaceLayout> {
        let layout = Self::calculate_surface_layout(surface.width, surface.height, surface.format)?;
        layout.validate_buffer_len(surface.data.len())?;
        Ok(layout)
    }

    /// Validates dimensions, primary row stride, and payload buffer length for a raw or decoded video surface.
    ///
    /// When `stride` exceeds plane 0's tightly-packed row stride, subsequent plane offsets in the returned
    /// [`TextureSurfaceLayout`] are shifted to account for plane 0's strided byte size (`stride * height`).
    pub fn validate_stride_and_buffer(
        width: u32,
        height: u32,
        stride: u32,
        pixel_format: PixelFormat,
        buffer_len: usize,
    ) -> Result<TextureSurfaceLayout> {
        let layout = Self::calculate_surface_layout(width, height, pixel_format)?;
        layout.validate_stride(0, stride)?;

        let mut plane_strides = Vec::with_capacity(layout.planes.len());
        plane_strides.push(stride);
        for plane in layout.planes.iter().skip(1) {
            plane_strides.push(plane.unpadded_bytes_per_row);
        }

        Self::validate_multiplanar_strides_and_buffer(
            width,
            height,
            &plane_strides,
            pixel_format,
            buffer_len,
        )
    }

    /// Validates dimensions, explicit per-plane row strides, and payload buffer length across all planes,
    /// returning a [`TextureSurfaceLayout`] with updated plane offsets matching the strided buffer layout.
    pub fn validate_multiplanar_strides_and_buffer(
        width: u32,
        height: u32,
        plane_strides: &[u32],
        pixel_format: PixelFormat,
        buffer_len: usize,
    ) -> Result<TextureSurfaceLayout> {
        let mut layout = Self::calculate_surface_layout(width, height, pixel_format)?;
        if plane_strides.len() != layout.planes.len() {
            bail!(
                "Expected {} plane strides for {:?}, but got {}",
                layout.planes.len(),
                pixel_format,
                plane_strides.len()
            );
        }

        let mut running_offset = 0usize;
        for (idx, (&stride, plane)) in plane_strides
            .iter()
            .zip(layout.planes.iter_mut())
            .enumerate()
        {
            if stride < plane.unpadded_bytes_per_row {
                bail!(
                    "Plane {} stride ({}) is smaller than minimum required row stride ({}) for {:?} {}x{}",
                    idx,
                    stride,
                    plane.unpadded_bytes_per_row,
                    pixel_format,
                    width,
                    height
                );
            }
            let strided_plane_bytes = (stride as usize)
                .checked_mul(plane.height as usize)
                .ok_or_else(|| anyhow::anyhow!("Strided plane {idx} byte size overflow"))?;
            plane.byte_offset = running_offset;
            running_offset = running_offset
                .checked_add(strided_plane_bytes)
                .ok_or_else(|| anyhow::anyhow!("Total required strided buffer size overflow"))?;
        }

        if buffer_len < running_offset {
            bail!(
                "Buffer length ({}) is smaller than required strided size ({}) for {:?} {}x{} (strides={:?})",
                buffer_len,
                running_offset,
                pixel_format,
                width,
                height,
                plane_strides
            );
        }

        Ok(layout)
    }

    /// Repacks a tightly-packed plane slice into a `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (`256`-byte) row-aligned buffer.
    pub fn pack_plane_to_wgpu_alignment(
        plane: &TexturePlaneLayout,
        src_data: &[u8],
    ) -> Result<Vec<u8>> {
        Self::pack_strided_plane_to_wgpu_alignment(plane, src_data, plane.unpadded_bytes_per_row)
    }

    /// Repacks a plane slice with custom source row pitch (`src_stride`) into a
    /// `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (`256`-byte) row-aligned buffer suitable for `Queue::write_texture`.
    pub fn pack_strided_plane_to_wgpu_alignment(
        plane: &TexturePlaneLayout,
        src_data: &[u8],
        src_stride: u32,
    ) -> Result<Vec<u8>> {
        if src_stride < plane.unpadded_bytes_per_row {
            bail!(
                "Source stride ({}) is smaller than plane {} minimum row bytes ({})",
                src_stride,
                plane.plane_index,
                plane.unpadded_bytes_per_row
            );
        }

        let strided_plane_bytes = (src_stride as usize)
            .checked_mul(plane.height as usize)
            .ok_or_else(|| anyhow::anyhow!("Plane {} strided size overflow", plane.plane_index))?;
        let required_end = plane
            .byte_offset
            .checked_add(strided_plane_bytes)
            .ok_or_else(|| anyhow::anyhow!("Plane slice end overflow"))?;
        if src_data.len() < required_end {
            bail!(
                "Source buffer length ({}) is smaller than plane {} end offset ({})",
                src_data.len(),
                plane.plane_index,
                required_end
            );
        }

        let plane_slice = &src_data[plane.byte_offset..required_end];
        if src_stride == plane.aligned_bytes_per_row
            && plane.unpadded_bytes_per_row == plane.aligned_bytes_per_row
        {
            return Ok(plane_slice.to_vec());
        }

        let unpadded_row = plane.unpadded_bytes_per_row as usize;
        let src_row_pitch = src_stride as usize;
        let aligned_row = plane.aligned_bytes_per_row as usize;
        let mut aligned_buf = vec![0u8; plane.aligned_byte_size];

        for row in 0..(plane.height as usize) {
            let src_start = row * src_row_pitch;
            let dst_start = row * aligned_row;
            aligned_buf[dst_start..dst_start + unpadded_row]
                .copy_from_slice(&plane_slice[src_start..src_start + unpadded_row]);
        }

        Ok(aligned_buf)
    }

    /// Builds a `wgpu::TextureDescriptor` for uploading and sampling a specific [`TexturePlaneLayout`].
    pub fn texture_descriptor<'a>(
        plane: &TexturePlaneLayout,
        label: Option<&'a str>,
    ) -> wgpu::TextureDescriptor<'a> {
        wgpu::TextureDescriptor {
            label,
            size: plane.extent_3d(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: plane.wgpu_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }
    }

    /// Strips WGSL line comments (`// ...`) and nested block comments (`/* ... */`),
    /// returning an error if a block comment is left unclosed.
    fn strip_wgsl_comments(source: &str) -> Result<String> {
        let mut out = String::with_capacity(source.len());
        let mut chars = source.chars().peekable();
        let mut block_depth = 0u32;

        while let Some(ch) = chars.next() {
            if block_depth > 0 {
                if ch == '/' && chars.peek() == Some(&'*') {
                    let _ = chars.next();
                    block_depth += 1;
                } else if ch == '*' && chars.peek() == Some(&'/') {
                    let _ = chars.next();
                    block_depth -= 1;
                    if block_depth == 0 {
                        out.push(' ');
                    }
                } else if ch == '\n' {
                    out.push('\n');
                }
                continue;
            }

            if ch == '/' {
                if chars.peek() == Some(&'/') {
                    let _ = chars.next();
                    for next_ch in chars.by_ref() {
                        if next_ch == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                } else if chars.peek() == Some(&'*') {
                    let _ = chars.next();
                    block_depth += 1;
                    continue;
                }
            }

            out.push(ch);
        }

        if block_depth != 0 {
            bail!("WGSL video shader validation failed: unclosed block comment `/*`");
        }

        Ok(out)
    }

    /// Validates WGSL video surface shader source structure, entry points, bindings, and delimiter balance,
    /// returning a ready-to-compile [`wgpu::ShaderModuleDescriptor`].
    pub fn validate_shader_source(wgsl_source: &str) -> Result<wgpu::ShaderModuleDescriptor<'_>> {
        let stripped = Self::strip_wgsl_comments(wgsl_source)?;
        let trimmed = stripped.trim();
        if trimmed.is_empty() {
            bail!("WGSL video surface shader source must not be empty");
        }

        let required_tokens = [
            ("@vertex", "vertex stage attribute"),
            ("fn vs_main", "vertex entry point `vs_main`"),
            ("@fragment", "fragment stage attribute"),
            ("fn fs_main", "fragment entry point `fs_main`"),
            (
                "@group(0) @binding(0)",
                "texture binding `@group(0) @binding(0)`",
            ),
            (
                "texture_2d<f32>",
                "sampled 2D texture uniform `texture_2d<f32>`",
            ),
            (
                "@group(0) @binding(1)",
                "sampler binding `@group(0) @binding(1)`",
            ),
            ("sampler", "texture sampler uniform"),
            ("textureSample(", "fragment texture sampling call"),
        ];

        for (token, description) in required_tokens {
            if !trimmed.contains(token) {
                bail!(
                    "WGSL video shader validation failed: missing required {} (`{}`)",
                    description,
                    token
                );
            }
        }

        let mut brace_depth = 0i32;
        let mut paren_depth = 0i32;
        let mut bracket_depth = 0i32;
        for ch in trimmed.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if brace_depth < 0 {
                        bail!("WGSL video shader validation failed: unmatched closing brace `}}`");
                    }
                }
                '(' => paren_depth += 1,
                ')' => {
                    paren_depth -= 1;
                    if paren_depth < 0 {
                        bail!(
                            "WGSL video shader validation failed: unmatched closing parenthesis `)`"
                        );
                    }
                }
                '[' => bracket_depth += 1,
                ']' => {
                    bracket_depth -= 1;
                    if bracket_depth < 0 {
                        bail!("WGSL video shader validation failed: unmatched closing bracket `]`");
                    }
                }
                _ => {}
            }
        }

        if brace_depth != 0 {
            bail!(
                "WGSL video shader validation failed: unbalanced braces (net depth: {})",
                brace_depth
            );
        }
        if paren_depth != 0 {
            bail!(
                "WGSL video shader validation failed: unbalanced parentheses (net depth: {})",
                paren_depth
            );
        }
        if bracket_depth != 0 {
            bail!(
                "WGSL video shader validation failed: unbalanced brackets (net depth: {})",
                bracket_depth
            );
        }

        Ok(wgpu::ShaderModuleDescriptor {
            label: Some("rferram_video_surface_shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(wgsl_source)),
        })
    }

    /// Validates the built-in [`VIDEO_SURFACE_SHADER_WGSL`] constant and returns its [`wgpu::ShaderModuleDescriptor`].
    pub fn validate_default_shader() -> Result<wgpu::ShaderModuleDescriptor<'static>> {
        Self::validate_shader_source(VIDEO_SURFACE_SHADER_WGSL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_surface_shader_wgsl_validation() {
        let descriptor = WgpuRenderPipeline::validate_default_shader()
            .expect("Built-in VIDEO_SURFACE_SHADER_WGSL must pass validation");
        assert_eq!(descriptor.label, Some("rferram_video_surface_shader"));
        match descriptor.source {
            wgpu::ShaderSource::Wgsl(ref cow) => {
                assert!(cow.contains("fn vs_main"));
                assert!(cow.contains("fn fs_main"));
            }
            _ => panic!("Expected WGSL shader source"),
        }

        // Valid shader with line and nested block comments containing unmatched delimiters must pass
        let commented_valid = format!(
            "// 1) Header comment with unmatched paren and brace {{\n/* outer /* nested ) }} */ comment */\n{VIDEO_SURFACE_SHADER_WGSL}"
        );
        assert!(WgpuRenderPipeline::validate_shader_source(&commented_valid).is_ok());

        // Empty shader source must fail
        assert!(WgpuRenderPipeline::validate_shader_source("   ").is_err());

        // Commented-out fragment entry point must fail
        let commented_out_fs =
            VIDEO_SURFACE_SHADER_WGSL.replace("fn fs_main", "// fn fs_main\nfn other_fs");
        assert!(WgpuRenderPipeline::validate_shader_source(&commented_out_fs).is_err());

        // Unclosed block comment must fail
        let unclosed_comment = format!("{VIDEO_SURFACE_SHADER_WGSL}\n/* unclosed");
        assert!(WgpuRenderPipeline::validate_shader_source(&unclosed_comment).is_err());

        // Missing fragment entry point must fail
        let missing_fs = VIDEO_SURFACE_SHADER_WGSL.replace("fn fs_main", "fn fragment_entry");
        assert!(WgpuRenderPipeline::validate_shader_source(&missing_fs).is_err());

        // Missing binding must fail
        let missing_binding =
            VIDEO_SURFACE_SHADER_WGSL.replace("@group(0) @binding(1)", "@group(0) @binding(2)");
        assert!(WgpuRenderPipeline::validate_shader_source(&missing_binding).is_err());

        // Unbalanced braces or brackets must fail
        let unbalanced = format!("{VIDEO_SURFACE_SHADER_WGSL}\n{{");
        assert!(WgpuRenderPipeline::validate_shader_source(&unbalanced).is_err());
        let unbalanced_bracket = format!("{VIDEO_SURFACE_SHADER_WGSL}\n]");
        assert!(WgpuRenderPipeline::validate_shader_source(&unbalanced_bracket).is_err());
    }

    #[test]
    fn test_bgra8_and_rgba8_layout_and_stride_alignment() {
        // 1920x1080 BGRA8: 1920 * 4 = 7680 (already a multiple of 256: 7680 = 30 * 256)
        let bgra_1080p =
            WgpuRenderPipeline::calculate_surface_layout(1920, 1080, PixelFormat::Bgra8).unwrap();
        assert_eq!(bgra_1080p.plane_count(), 1);
        assert_eq!(
            bgra_1080p.primary_wgpu_format,
            wgpu::TextureFormat::Bgra8UnormSrgb
        );
        assert_eq!(bgra_1080p.primary_unpadded_stride(), 7680);
        assert_eq!(bgra_1080p.primary_aligned_stride(), 7680);
        assert_eq!(bgra_1080p.total_unpadded_bytes, 1920 * 1080 * 4);
        assert_eq!(bgra_1080p.total_aligned_bytes, 1920 * 1080 * 4);

        // 1366x768 RGBA8: 1366 * 4 = 5464; next multiple of 256 is 5632 (22 * 256)
        let rgba_768p =
            WgpuRenderPipeline::calculate_surface_layout(1366, 768, PixelFormat::Rgba8).unwrap();
        assert_eq!(rgba_768p.plane_count(), 1);
        assert_eq!(
            rgba_768p.primary_wgpu_format,
            wgpu::TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(rgba_768p.primary_unpadded_stride(), 5464);
        assert_eq!(rgba_768p.primary_aligned_stride(), 5632);
        assert_eq!(rgba_768p.total_unpadded_bytes, 5464 * 768);
        assert_eq!(rgba_768p.total_aligned_bytes, 5632 * 768);
    }

    #[test]
    fn test_nv12_and_yuv420p_and_yuv444p_multiplanar_layouts() {
        // 1920x1080 NV12:
        // Plane 0 (Y): 1920x1080, 1 Bpp -> unpadded stride 1920, aligned stride 2048 (8 * 256)
        // Plane 1 (UV): 960x540, 2 Bpp -> unpadded stride 1920, aligned stride 2048
        let nv12 =
            WgpuRenderPipeline::calculate_surface_layout(1920, 1080, PixelFormat::NV12).unwrap();
        assert_eq!(nv12.plane_count(), 2);
        assert_eq!(nv12.primary_wgpu_format, wgpu::TextureFormat::NV12);
        assert_eq!(nv12.planes[0].width, 1920);
        assert_eq!(nv12.planes[0].height, 1080);
        assert_eq!(nv12.planes[0].unpadded_bytes_per_row, 1920);
        assert_eq!(nv12.planes[0].aligned_bytes_per_row, 2048);
        assert_eq!(nv12.planes[0].unpadded_byte_size, 1920 * 1080);
        assert_eq!(nv12.planes[1].width, 960);
        assert_eq!(nv12.planes[1].height, 540);
        assert_eq!(nv12.planes[1].bytes_per_pixel, 2);
        assert_eq!(nv12.planes[1].unpadded_bytes_per_row, 1920);
        assert_eq!(nv12.planes[1].aligned_bytes_per_row, 2048);
        assert_eq!(nv12.planes[1].byte_offset, 1920 * 1080);
        assert_eq!(nv12.planes[1].unpadded_byte_size, 1920 * 540);
        assert_eq!(nv12.total_unpadded_bytes, (1920 * 1080) * 3 / 2);

        // 1920x1080 YUV420P:
        // Plane 0 (Y): 1920x1080, 1 Bpp -> stride 1920 (aligned 2048)
        // Plane 1 (U): 960x540, 1 Bpp -> stride 960 (aligned 1024)
        // Plane 2 (V): 960x540, 1 Bpp -> stride 960 (aligned 1024)
        let yuv420 =
            WgpuRenderPipeline::calculate_surface_layout(1920, 1080, PixelFormat::YUV420P).unwrap();
        assert_eq!(yuv420.plane_count(), 3);
        assert_eq!(yuv420.planes[0].unpadded_byte_size, 1920 * 1080);
        assert_eq!(yuv420.planes[1].width, 960);
        assert_eq!(yuv420.planes[1].height, 540);
        assert_eq!(yuv420.planes[1].unpadded_bytes_per_row, 960);
        assert_eq!(yuv420.planes[1].aligned_bytes_per_row, 1024);
        assert_eq!(yuv420.planes[1].byte_offset, 1920 * 1080);
        assert_eq!(yuv420.planes[2].byte_offset, 1920 * 1080 + 960 * 540);
        assert_eq!(yuv420.total_unpadded_bytes, (1920 * 1080) * 3 / 2);

        // 1920x1080 YUV444P: 3 full-resolution planes
        let yuv444 =
            WgpuRenderPipeline::calculate_surface_layout(1920, 1080, PixelFormat::YUV444P).unwrap();
        assert_eq!(yuv444.plane_count(), 3);
        assert_eq!(yuv444.total_unpadded_bytes, 1920 * 1080 * 3);
    }

    #[test]
    fn test_buffer_stride_and_texture_size_validation() {
        // Zero and oversized dimensions must fail
        assert!(WgpuRenderPipeline::validate_dimensions(0, 1080).is_err());
        assert!(WgpuRenderPipeline::validate_dimensions(1920, 0).is_err());
        assert!(
            WgpuRenderPipeline::validate_dimensions(MAX_TEXTURE_DIMENSION_2D + 1, 1080).is_err()
        );
        assert!(
            WgpuRenderPipeline::validate_dimensions(1920, MAX_TEXTURE_DIMENSION_2D + 1).is_err()
        );

        // Under-sized stride for BGRA8 (requires >= 640 * 4 = 2560) must fail
        assert!(
            WgpuRenderPipeline::validate_stride_and_buffer(
                640,
                480,
                2000,
                PixelFormat::Bgra8,
                640 * 480 * 4
            )
            .is_err()
        );

        // Under-sized buffer for valid stride must fail
        assert!(
            WgpuRenderPipeline::validate_stride_and_buffer(
                640,
                480,
                2560,
                PixelFormat::Bgra8,
                640 * 480 * 4 - 1
            )
            .is_err()
        );

        // Valid stride and buffer length must succeed
        let valid = WgpuRenderPipeline::validate_stride_and_buffer(
            640,
            480,
            2560,
            PixelFormat::Bgra8,
            640 * 480 * 4,
        )
        .unwrap();
        assert_eq!(valid.primary_unpadded_stride(), 2560);

        // Strided multi-planar NV12 (plane 0 stride 2048 > width 1920) must shift plane 1 offset
        let strided_nv12 = WgpuRenderPipeline::validate_stride_and_buffer(
            1920,
            1080,
            2048,
            PixelFormat::NV12,
            2048 * 1080 + 1920 * 540,
        )
        .unwrap();
        assert_eq!(strided_nv12.planes[0].byte_offset, 0);
        assert_eq!(strided_nv12.planes[1].byte_offset, 2048 * 1080);

        // Explicit multi-planar strides for YUV420P (e.g. [256, 128, 128] for 100x4)
        let strided_yuv420 = WgpuRenderPipeline::validate_multiplanar_strides_and_buffer(
            100,
            4,
            &[256, 128, 128],
            PixelFormat::YUV420P,
            256 * 4 + 128 * 2 + 128 * 2,
        )
        .unwrap();
        assert_eq!(strided_yuv420.planes[0].byte_offset, 0);
        assert_eq!(strided_yuv420.planes[1].byte_offset, 256 * 4);
        assert_eq!(strided_yuv420.planes[2].byte_offset, 256 * 4 + 128 * 2);

        // Wrong number of plane strides must fail
        assert!(
            WgpuRenderPipeline::validate_multiplanar_strides_and_buffer(
                100,
                4,
                &[256, 128],
                PixelFormat::YUV420P,
                4096,
            )
            .is_err()
        );

        // DecodedFrameSurface validation and plane repacking to 256-byte alignment
        let surface = DecodedFrameSurface {
            frame_index: 42,
            width: 100,
            height: 4,
            format: PixelFormat::Rgba8,
            data: vec![0x7F; 100 * 4 * 4],
            timestamp_us: 16_666,
        };
        let layout = surface.validate().unwrap();
        assert_eq!(layout.planes[0].unpadded_bytes_per_row, 400);
        assert_eq!(layout.planes[0].aligned_bytes_per_row, 512);

        let aligned =
            WgpuRenderPipeline::pack_plane_to_wgpu_alignment(&layout.planes[0], &surface.data)
                .unwrap();
        assert_eq!(aligned.len(), 512 * 4);
        assert_eq!(&aligned[0..400], &surface.data[0..400]);
        assert_eq!(&aligned[400..512], &[0u8; 112]);

        // Custom strided plane repacking (e.g. src_stride = 448 -> aligned 512)
        let mut custom_strided = vec![0xFFu8; 448 * 4];
        for row in 0..4 {
            for col in 0..400 {
                custom_strided[row * 448 + col] = 0x33;
            }
        }
        let repacked = WgpuRenderPipeline::pack_strided_plane_to_wgpu_alignment(
            &layout.planes[0],
            &custom_strided,
            448,
        )
        .unwrap();
        assert_eq!(repacked.len(), 512 * 4);
        assert_eq!(&repacked[0..400], &[0x33u8; 400]);
        assert_eq!(&repacked[400..512], &[0u8; 112]);

        // Truncated source buffer in pack_plane_to_wgpu_alignment must fail
        assert!(
            WgpuRenderPipeline::pack_plane_to_wgpu_alignment(
                &layout.planes[0],
                &surface.data[..100]
            )
            .is_err()
        );
    }

    #[test]
    fn test_pipeline_reconfigure_and_edge_cases() {
        let mut pipeline = WgpuRenderPipeline::new(1920, 1080, PixelFormat::Bgra8).unwrap();
        assert_eq!(pipeline.width, 1920);
        assert_eq!(pipeline.height, 1080);
        assert_eq!(pipeline.layout.plane_count(), 1);

        // Reconfigure to 2560x1440 NV12
        let new_layout = pipeline
            .reconfigure(2560, 1440, PixelFormat::NV12)
            .unwrap()
            .clone();
        assert_eq!(pipeline.width, 2560);
        assert_eq!(pipeline.height, 1440);
        assert_eq!(pipeline.pixel_format, PixelFormat::NV12);
        assert_eq!(new_layout.plane_count(), 2);

        // Boundary 1x1 and 3x3 odd-dimension chroma subsampling
        let odd_nv12 =
            WgpuRenderPipeline::calculate_surface_layout(3, 3, PixelFormat::NV12).unwrap();
        assert_eq!(odd_nv12.planes[0].width, 3);
        assert_eq!(odd_nv12.planes[0].height, 3);
        assert_eq!(odd_nv12.planes[1].width, 2);
        assert_eq!(odd_nv12.planes[1].height, 2);
        assert_eq!(odd_nv12.planes[1].unpadded_bytes_per_row, 4);

        // Maximum 2D dimension boundary (16384x16384)
        let max_layout = WgpuRenderPipeline::calculate_surface_layout(
            MAX_TEXTURE_DIMENSION_2D,
            MAX_TEXTURE_DIMENSION_2D,
            PixelFormat::Bgra8,
        )
        .unwrap();
        assert_eq!(
            max_layout.total_unpadded_bytes,
            (MAX_TEXTURE_DIMENSION_2D as usize) * (MAX_TEXTURE_DIMENSION_2D as usize) * 4
        );

        // Out-of-bounds plane index in validate_stride must fail
        assert!(max_layout.validate_stride(1, 65536).is_err());

        // Verify wgpu descriptor and TexelCopyBufferLayout helpers
        let tex_desc = WgpuRenderPipeline::texture_descriptor(&odd_nv12.planes[1], Some("uv_tex"));
        assert_eq!(tex_desc.label, Some("uv_tex"));
        assert_eq!(tex_desc.size.width, 2);
        assert_eq!(tex_desc.size.height, 2);
        assert_eq!(tex_desc.format, wgpu::TextureFormat::Rg8Unorm);

        let copy_unaligned = odd_nv12.planes[1].texel_copy_buffer_layout(false);
        assert_eq!(copy_unaligned.offset, 9);
        assert_eq!(copy_unaligned.bytes_per_row, Some(4));
        assert_eq!(copy_unaligned.rows_per_image, Some(2));

        let copy_aligned = odd_nv12.planes[1].texel_copy_buffer_layout(true);
        assert_eq!(copy_aligned.offset, 0);
        assert_eq!(copy_aligned.bytes_per_row, Some(256));
        assert_eq!(copy_aligned.rows_per_image, Some(2));

        assert_eq!(
            WgpuRenderPipeline::minimum_row_stride(256, PixelFormat::Rgba8).unwrap(),
            1024
        );
        assert_eq!(
            WgpuRenderPipeline::aligned_row_stride(100, PixelFormat::Rgba8).unwrap(),
            512
        );
        assert_eq!(
            WgpuRenderPipeline::expected_frame_bytes(64, 64, PixelFormat::YUV420P).unwrap(),
            64 * 64 + 32 * 32 * 2
        );
    }

    #[tokio::test]
    async fn test_wgpu_device_shader_compilation_and_texture_upload() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
        else {
            // Headless environment without a Vulkan/Metal/DX12 adapter
            return;
        };

        let Ok((device, queue)) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
        else {
            return;
        };

        let shader_desc = WgpuRenderPipeline::validate_default_shader().unwrap();
        let _shader_module = device.create_shader_module(shader_desc);

        for format in [
            PixelFormat::Bgra8,
            PixelFormat::Rgba8,
            PixelFormat::NV12,
            PixelFormat::YUV420P,
        ] {
            let layout = WgpuRenderPipeline::calculate_surface_layout(100, 64, format).unwrap();
            let raw_frame = vec![0x80u8; layout.total_unpadded_bytes];
            for plane in &layout.planes {
                let tex =
                    device.create_texture(&WgpuRenderPipeline::texture_descriptor(plane, None));
                let aligned =
                    WgpuRenderPipeline::pack_plane_to_wgpu_alignment(plane, &raw_frame).unwrap();
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &aligned,
                    plane.texel_copy_buffer_layout(true),
                    plane.extent_3d(),
                );
            }
        }
        queue.submit([]);
    }
}
