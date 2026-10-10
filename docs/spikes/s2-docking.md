# S2: docking layout and headless interface tests

Run 2026-10-11 on Windows 10, rustc 1.99.

**Decision: use `egui_tiles` for the dockable workspace**, kept behind one
layout module so it can be swapped. Headless interface tests with
`egui_kittest` work.

## What was run

The same small workspace (two image tabs; Info, Histogram, History and
Results panels; a Window menu; a command palette; light and dark themes;
layout save, reset and restore) was built on both `egui_dock` 0.21.1 and
`egui_tiles` 0.17.1. 19 headless tests pass with no window, covering the
palette, closing and re-opening panels, dragging a tab to another group, and
layout round trips. Both binaries were launched and screenshots of their
windows inspected.

Not checked: real mouse use in the windows, macOS, Linux, any CI runner,
release builds, HiDPI snapshots.

## Comparison

Both crates resolve against egui 0.36.2 with no pinning, and both released
for egui 0.36 on the same day as egui.

| | egui_dock | egui_tiles |
|---|---|---|
| Layout | Binary splits | N-ary splits, grids and tabs (suits side-by-side images) |
| Testability | Tabs and close buttons are unlabelled in the accessibility tree; tabs can be named with a workaround, close buttons cannot | Tabs and close buttons are labelled buttons out of the box |
| Close and re-open | Forgets where a closed tab was | Can hide a pane and keep its slot, so re-opening restores its position |
| Floating panels | Yes, as windows inside the main window | None |
| Saved layout | Byte-stable JSON | Lossless JSON, not byte-stable |
| Maintainers | Community | The egui and kittest maintainers |

`egui_tiles` wins on testability and on panels keeping their place. Its cost
is no floating panels; if those become necessary the layout module is the
only code that changes (the two backends were 109 and 164 lines).

## Notes for implementation

- Store only a small pane id in the layout tree; keep documents and view
  state in the application model. Treat a saved layout as an opaque,
  versioned string and fall back to the default if it does not load.
- `egui_tiles`: hiding a pane from inside `on_tab_close` trips a debug
  assertion. Defer the hide until after `tree.ui()`.
- Command palette: `egui::Modal`, with Esc, Enter and arrow keys consumed
  before the text field sees them.

## Test patterns that worked

```rust
let mut h = Harness::builder().with_size(vec2(1000., 650.))
    .build_ui_state(|ui, app: &mut App| app.ui(ui), App::default());
h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::P); h.run();
h.get_by_role(Role::TextInput).type_text("gaus"); h.run();
h.key_press(Key::Enter); h.run();
h.get_by_role_and_label(Role::Button, "Window").click(); h.run();
```

- Send `Modifiers::COMMAND`, not `CTRL`, for shortcuts.
- Use `get_by_role_and_label`: a tab and a menu item can share a label.

## Snapshot tests and CI

- Image snapshots need a wgpu adapter but no display and no real GPU; the
  software adapter is chosen by default.
- Snapshots are deterministic on one adapter but differ between adapters
  (76 to 1721 pixels between the software renderer and the AMD GPU). Keep
  reference images for one renderer only, and run accessibility-tree tests
  everywhere.
- In debug builds, creating the headless DX12 instance crashed the test
  process on this machine until wgpu's debug flag was removed in the test
  setup (or `WGPU_DEBUG=0` was set). The cause is not established.

Clean debug build of the spike: 7 minutes.
