# S1: 16-bit display with window/level and LUT on wgpu inside egui

Run 2026-10-10 on Windows 10, rustc 1.99, AMD Radeon Pro 5300M.

**Result: works.** A plane is uploaded as an integer texture, and a fragment
shader applies the display range and a 256-entry lookup table. It is drawn by
a custom pipeline inside egui's render pass.

## What was run

- Offscreen tests comparing GPU output with a CPU reference: 7 of 7 passed on
  Vulkan, DX12, OpenGL 4.6 and the Microsoft software renderer (WARP).
- `R8Uint`, `R16Uint` and `R32Float` planes, nearest and manual bilinear.
- A 5000 x 3000 16-bit plane: upload, render and read back in 60 to 95 ms on
  Vulkan and DX12, about 170 to 410 ms on OpenGL, 2.3 s on WARP. The CPU
  reference took about 380 ms.
- An eframe app with the image in a panel, launched on Vulkan, DX12 and
  OpenGL with no validation errors. Screenshots taken by the app matched the
  CPU reference exactly on Vulkan and DX12 and within 1 level on OpenGL.

Not checked: the real on-screen window (screenshots came from egui's own
capture path), interactive slider use, tiling, display scales other than
1.25, behaviour with no graphics adapter at all.

## Versions

eframe, egui, egui-wgpu 0.36.2 with wgpu 30.0.1 and winit 0.30.13. No crate
in the tree needs cmake. Use the `egui_wgpu::wgpu` re-export rather than a
direct wgpu dependency so the versions cannot drift.

`eframe = { version = "0.36.2", default-features = false, features = ["wgpu", "default_fonts"] }`

## API notes for this egui and wgpu

```rust
// The app trait method is `ui` and receives a root Ui.
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) { .. }
}
// Panels: Panel::left(id).show(ui, ..), CentralPanel::default().show(ui, ..)

// Device, queue and target format, once, in the app creator:
let rs = cc.wgpu_render_state.as_ref().unwrap();
rs.renderer.write().callback_resources.insert(resources);

// Paint callback (must be Send + Sync):
impl egui_wgpu::CallbackTrait for PlaneCallback {
    fn prepare(&self, device, queue, screen, encoder, resources) -> Vec<wgpu::CommandBuffer>;
    fn paint(&self, info, pass: &mut wgpu::RenderPass<'static>, resources);
}
ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback));
```

- egui sets the viewport to the callback rectangle, so draw one
  full-viewport triangle.
- Bind integer planes as `TextureSampleType::Uint` and float planes as
  `Float { filterable: false }`; read both with `textureLoad`. There is no
  sampler, so zoom filtering is done by hand in the shader.
- The target format differs by backend (`Bgra8Unorm` on Vulkan and DX12,
  `Rgba8Unorm` on OpenGL). Both are non-sRGB, so LUT bytes pass through
  unchanged; the LUT texture must be `Rgba8Unorm`, not an sRGB format.
- `write_texture` needs no row alignment; reading a texture back needs rows
  padded to 256 bytes.

## Findings that shape the design

1. **Keep this pipeline.** It needs no optional GPU features. Changing the
   display range or LUT is a small uniform or 1 KB texture write, with no
   re-upload of the image.
2. **Tile from the start.** eframe's default device limits textures to 8192
   pixels a side even when the adapter allows 16384. Read
   `device.limits().max_texture_dimension_2d` at startup and split larger
   planes into tiles with a one-texel border. Creating an oversize texture
   gives a catchable validation error, not a crash.
3. **Use integer arithmetic in the shader for integer planes.** With float
   arithmetic the GPU was occasionally one LUT entry away from the CPU (for
   example 141 of 15,000,000 pixels). That is within 1 display level for grey
   LUTs but up to 7 for colour LUTs. The shader must implement the mapping in
   `calipix-render` exactly (see `docs/compat.md`), which for integer planes
   means integer maths.
4. **Detect the software renderer.** `adapter.get_info().device_type ==
   DeviceType::Cpu` identifies WARP. It is correct but 25 to 35 times slower;
   prefer the CPU renderer there.

Clean build of the spike: just over 5 minutes (332 packages).
