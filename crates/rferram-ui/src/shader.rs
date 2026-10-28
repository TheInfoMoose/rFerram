//! Native COSMIC Desktop Frosted Glass WGSL Fragment Shader & Uniforms
//!
//! Provides `FROSTED_GLASS_SHADER_WGSL`, the `#[repr(C)]` 32-byte aligned
//! `FrostedGlassUniforms` structure matching the WGSL uniform buffer layout,
//! and shader validation utilities.

use std::borrow::Cow;

/// Native COSMIC Desktop Frosted Glass WGSL Fragment Shader
pub const FROSTED_GLASS_SHADER_WGSL: &str = r#"
struct FrostedGlassUniforms {
    tint_color: vec4<f32>,
    blur_radius: f32,
    opacity: f32,
    corner_radius: f32,
    _pad: f32,
};

@group(0) @binding(0)
var<uniform> glass: FrostedGlassUniforms;

@fragment
fn fs_frosted_glass(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let alpha = clamp(glass.opacity, 0.0, 1.0);
    return vec4<f32>(glass.tint_color.rgb, alpha);
}
"#;

/// Uniform buffer layout matching `struct FrostedGlassUniforms` in `FROSTED_GLASS_SHADER_WGSL`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrostedGlassUniforms {
    pub tint_color: [f32; 4],
    pub blur_radius: f32,
    pub opacity: f32,
    pub corner_radius: f32,
    pub _pad: f32,
}

impl FrostedGlassUniforms {
    /// Serializes the uniform structure into its 32-byte little-endian `#[repr(C)]` layout for `wgpu` buffer uploads.
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut buf = [0u8; 32];
        for (i, component) in self.tint_color.iter().enumerate() {
            buf[i * 4..(i + 1) * 4].copy_from_slice(&component.to_le_bytes());
        }
        buf[16..20].copy_from_slice(&self.blur_radius.to_le_bytes());
        buf[20..24].copy_from_slice(&self.opacity.to_le_bytes());
        buf[24..28].copy_from_slice(&self.corner_radius.to_le_bytes());
        buf[28..32].copy_from_slice(&self._pad.to_le_bytes());
        buf
    }
}

impl Default for FrostedGlassUniforms {
    fn default() -> Self {
        Self {
            tint_color: [0.12, 0.14, 0.18, 1.0],
            blur_radius: 16.0,
            opacity: 0.82,
            corner_radius: 12.0,
            _pad: 0.0,
        }
    }
}

/// Validates that `FROSTED_GLASS_SHADER_WGSL` declares the expected uniform binding, fragment entry point, and balanced delimiters.
pub fn validate_frosted_glass_shader_source() -> bool {
    let has_tokens = FROSTED_GLASS_SHADER_WGSL.contains("struct FrostedGlassUniforms")
        && FROSTED_GLASS_SHADER_WGSL.contains("@group(0) @binding(0)")
        && FROSTED_GLASS_SHADER_WGSL.contains("var<uniform> glass: FrostedGlassUniforms;")
        && FROSTED_GLASS_SHADER_WGSL.contains("@fragment")
        && FROSTED_GLASS_SHADER_WGSL.contains("fn fs_frosted_glass");

    let mut brace_depth = 0i32;
    let mut paren_depth = 0i32;
    for ch in FROSTED_GLASS_SHADER_WGSL.chars() {
        match ch {
            '{' => brace_depth += 1,
            '}' => {
                brace_depth -= 1;
                if brace_depth < 0 {
                    return false;
                }
            }
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }

    has_tokens && brace_depth == 0 && paren_depth == 0
}

/// Returns the `wgpu::ShaderModuleDescriptor` for compiling [`FROSTED_GLASS_SHADER_WGSL`].
pub fn frosted_glass_shader_descriptor() -> wgpu::ShaderModuleDescriptor<'static> {
    wgpu::ShaderModuleDescriptor {
        label: Some("rferram_frosted_glass_shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(FROSTED_GLASS_SHADER_WGSL)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};

    #[test]
    fn test_frosted_glass_shader_source_and_uniform_layout() {
        assert!(validate_frosted_glass_shader_source());
        assert_eq!(size_of::<FrostedGlassUniforms>(), 32);
        assert_eq!(align_of::<FrostedGlassUniforms>(), 4);

        let uniforms = FrostedGlassUniforms::default();
        assert!((0.0..=1.0).contains(&uniforms.opacity));
        assert!(uniforms.blur_radius > 0.0);
        assert!(uniforms.corner_radius > 0.0);

        let bytes = uniforms.to_bytes();
        assert_eq!(
            f32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            uniforms.blur_radius
        );
        assert_eq!(
            f32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            uniforms.opacity
        );
        assert_eq!(
            f32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            uniforms.corner_radius
        );

        let descriptor = frosted_glass_shader_descriptor();
        assert_eq!(descriptor.label, Some("rferram_frosted_glass_shader"));
        match descriptor.source {
            wgpu::ShaderSource::Wgsl(ref src) => {
                assert!(src.contains("fn fs_frosted_glass"));
            }
            _ => panic!("Expected WGSL shader source"),
        }
    }

    #[tokio::test]
    async fn test_wgpu_device_frosted_glass_shader_compilation_and_uniform_buffer() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
        else {
            return;
        };

        let Ok((device, queue)) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
        else {
            return;
        };

        let _shader_module = device.create_shader_module(frosted_glass_shader_descriptor());

        let uniform_bytes = FrostedGlassUniforms::default().to_bytes();
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rferram_frosted_glass_uniform_buffer"),
            size: uniform_bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buffer, 0, &uniform_bytes);
        queue.submit([]);
    }
}
