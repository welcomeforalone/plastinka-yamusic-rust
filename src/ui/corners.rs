//! Скруглённые углы окна без системной рамки в Linux.
//!
//! В Windows 11 углы окна без рамки скругляет сама система, в Linux — нет:
//! окно рисуется прямоугольником. Скругляем сами: окно делаем прозрачным
//! (`transparent`), а поверх всего кадра кладём слой [`eraser`]. Он ничего не
//! рисует, а стирает уже нарисованное в четырёх углах за дугой радиуса
//! [`RADIUS`] (с полупрозрачным краем, без лесенки). Так углы скругляются у
//! всего сразу: фона, полноэкранного плеера с размытой обложкой, диалогов,
//! подсветки кнопки «Закрыть» — без правки фона каждого слоя.
//!
//! Слой работает только с отрисовкой через wgpu. На программном рендере
//! (tiny-skia, он по умолчанию, см. `render`) окно не делаем прозрачным и слой
//! не кладём: углы прямые. Стереть их нечем: softbuffer в Wayland выводит кадр
//! без альфа-канала. Без композитора с прозрачностью (X11 без композитора)
//! стёртые углы видны чёрными.

use std::borrow::Cow;

use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::{Element, Length, Rectangle, mouse};

/// Скругляем ли углы на этой платформе. В Windows углы скругляет система.
pub const ENABLED: bool = cfg!(target_os = "linux");

/// Радиус скругления в пикселях экрана — как у окон Windows 11.
pub const RADIUS: f32 = 8.0;

/// Радиус углов окна в логических единицах вида. `scale` — масштаб вида
/// (Ctrl + = / −): радиус на экране от него не зависит. Развёрнутое и
/// полноэкранное окно прилегает к краям экрана — углы прямые.
pub fn radius(enabled: bool, maximized: bool, fullscreen: bool, scale: f32) -> f32 {
    if !enabled || maximized || fullscreen || scale <= 0.0 {
        return 0.0;
    }
    RADIUS / scale
}

/// Слой, который стирает углы под собой. Кладётся последним в стопку окна
/// и растягивается на всё окно. Мышь не ловит: края для изменения размера
/// и перетаскивание работают как без него. `None` — скруглять не нужно.
pub fn eraser<'a, Message: 'a>(radius: f32) -> Option<Element<'a, Message>> {
    if radius <= 0.0 {
        return None;
    }
    Some(
        iced::widget::shader(Eraser { radius })
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
    )
}

/// Программа слоя: состояния и событий нет, только рисование.
struct Eraser {
    radius: f32,
}

impl<Message> shader::Program<Message> for Eraser {
    type State = ();
    type Primitive = Corners;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> Corners {
        Corners {
            radius: self.radius,
        }
    }
}

/// Что стереть в кадре: углы прямоугольника слоя с этим радиусом
/// (в логических единицах).
#[derive(Debug)]
pub struct Corners {
    radius: f32,
}

/// Данные для шейдера: прямоугольник слоя и радиус, в физических пикселях.
fn uniforms(bounds: &Rectangle, scale: f32, radius: f32) -> [f32; 8] {
    [
        bounds.x * scale,
        bounds.y * scale,
        bounds.width * scale,
        bounds.height * scale,
        radius * scale,
        0.0,
        0.0,
        0.0,
    ]
}

/// Байты буфера для GPU (little-endian, как ждёт wgpu).
fn to_bytes(values: [f32; 8]) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let (chunks, _) = bytes.as_chunks_mut::<4>();
    for (chunk, value) in chunks.iter_mut().zip(values) {
        *chunk = value.to_le_bytes();
    }
    bytes
}

impl shader::Primitive for Corners {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        // Слой один на окно, окна рисуются по очереди: одного буфера хватает.
        let values = uniforms(bounds, viewport.scale_factor(), self.radius);
        queue.write_buffer(&pipeline.uniforms, 0, &to_bytes(values));
    }

    fn draw(&self, pipeline: &Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, &pipeline.bind_group, &[]);
        // Четыре угла по два треугольника.
        render_pass.draw(0..6, 0..4);
        true
    }
}

/// Конвейер wgpu: создаётся один раз на первом кадре со слоем.
pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("plastinka corners"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("plastinka corners uniforms"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("plastinka corners layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("plastinka corners bind group"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("plastinka corners pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        // Стирание: результат = нарисованное × (1 − маска). Цвет и
        // прозрачность умножаются одинаково, поэтому край дуги корректен и
        // для premultiplied-, и для обычной альфы.
        let erase = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("plastinka corners pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: erase,
                        alpha: erase,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            uniforms,
            bind_group,
        }
    }
}

/// Вершины — квадраты радиуса в углах слоя, фрагменты — маска «за дугой».
const SHADER: &str = r"
struct Uniforms {
    rect: vec4<f32>,
    radius: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

fn clamped_radius() -> f32 {
    return min(u.radius.x, min(u.rect.z, u.rect.w) * 0.5);
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex: u32,
    @builtin(instance_index) corner: u32,
) -> @builtin(position) vec4<f32> {
    var square = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let r = clamped_radius();
    let size = u.rect.zw;
    let right = f32(corner & 1u);
    let bottom = f32((corner >> 1u) & 1u);
    let at = vec2<f32>(right * (size.x - r), bottom * (size.y - r)) + square[vertex] * r;
    // Область вывода — прямоугольник слоя: координаты от его угла.
    let ndc = vec2<f32>(at.x / size.x * 2.0 - 1.0, 1.0 - at.y / size.y * 2.0);
    return vec4<f32>(ndc, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let r = clamped_radius();
    let half = u.rect.zw * 0.5;
    let local = position.xy - u.rect.xy;
    let q = abs(local - half) - (half - vec2<f32>(r, r));
    let outside = length(max(q, vec2<f32>(0.0, 0.0))) - r;
    let mask = clamp(outside + 0.5, 0.0, 1.0);
    return vec4<f32>(0.0, 0.0, 0.0, mask);
}
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_only_floating_window_on_supported_platform() {
        assert_eq!(radius(true, false, false, 1.0), RADIUS);
        // Развёрнутое и полноэкранное окно — прямые углы.
        assert_eq!(radius(true, true, false, 1.0), 0.0);
        assert_eq!(radius(true, false, true, 1.0), 0.0);
        assert_eq!(radius(true, true, true, 1.0), 0.0);
        // Платформа скругляет сама (Windows) — не трогаем.
        assert_eq!(radius(false, false, false, 1.0), 0.0);
    }

    #[test]
    fn radius_on_screen_does_not_depend_on_view_scale() {
        assert_eq!(radius(true, false, false, 2.0), RADIUS / 2.0);
        assert_eq!(radius(true, false, false, 0.5), RADIUS * 2.0);
        // Битый масштаб не даёт бесконечного радиуса.
        assert_eq!(radius(true, false, false, 0.0), 0.0);
    }

    #[test]
    fn eraser_is_absent_without_radius() {
        assert!(eraser::<()>(0.0).is_none());
        assert!(eraser::<()>(RADIUS).is_some());
    }

    #[test]
    fn uniforms_are_in_physical_pixels() {
        let bounds = Rectangle::new(iced::Point::new(1.0, 2.0), iced::Size::new(100.0, 50.0));
        assert_eq!(
            uniforms(&bounds, 1.5, 8.0),
            [1.5, 3.0, 150.0, 75.0, 12.0, 0.0, 0.0, 0.0]
        );
        let bytes = to_bytes([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(bytes.get(..4), Some(&1.0f32.to_le_bytes()[..]));
        assert!(
            bytes
                .get(4..)
                .is_some_and(|rest| rest.iter().all(|&b| b == 0))
        );
    }
}
