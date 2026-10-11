# Interface design

This is the specification every piece of the interface follows. It exists so
that ImageR2 feels like one application, however many people and extensions
contribute to it. Change this document first, then the code.

## Principles

1. **One window.** Images, results, history, the macro editor, the assistant
   and extensions are panels of one workspace. Nothing opens a separate
   top-level window unless the user tears it off.
2. **Looking never changes data.** Anything under *View* (zoom, brightness
   and contrast, lookup tables, channel visibility) changes only how an image
   is displayed. Anything that changes sample values is under *Process* and
   says so. ImageJ blurs this line (its B&C "Apply" button, for example);
   ImageR2 does not.
3. **Everything is a command.** If the interface can do it, it is in the
   command palette, the command line, macros and the agent tools, with the
   same name and parameters. No bespoke dialogs.
4. **Everything can be undone**, and the History panel shows what was done.
5. **Say what happened.** Every action ends with a status message, a visible
   result or an error that names the problem and the thing it concerns.
6. **ImageJ users can find things.** Every command that exists in ImageJ
   keeps its ImageJ name as a search alias in the palette, even where its
   menu location or title has changed.

## Window layout

```
+--------------------------------------------------------------+
| File  Edit  View  Image  Process  Analyze  Extensions  Window  Help |
+--------------------------------------------+-----------------+
|  [ image A ] [ image B ]                   |  Info           |
|                                            +-----------------+
|              image viewer                  |  Display        |
|                                            |                 |
|  C 1 2 3  Z ----o--------  T --o---------- +-----------------+
+--------------------------------------------+  Histogram      |
|  History | Results | Log                   |                 |
+--------------------------------------------+-----------------+
| status message                    x, y, value   zoom   image |
+--------------------------------------------------------------+
```

- **Centre:** open images as tabs. Tabs can be dragged side by side or into
  a grid to compare images; *Window > Show Images Side by Side* does it in
  one step.
- **Right:** *Info* (name, size, type, calibration), *Display* (display
  range, lookup table, channels) and *Histogram* for the active image. The
  column runs the full height of the window so that all three fit.
- **Bottom:** *History* (every command run, with undo position), *Results*
  (measurement tables) and *Log*.
- Every panel can be moved, hidden and shown again from the *Window* menu,
  and returns to where it was. *Window > Reset Layout* restores the default.
- The layout is remembered between sessions.

## Menus

Menus are generated from command definitions. A command's menu location is
chosen by what it does, using these rules:

| Menu | Holds | Examples |
|---|---|---|
| File | Getting images in and out | New, Open, Open Recent, Import, Save, Save As, Export, Close |
| Edit | Undo, clipboard, selections, preferences | Undo, Redo, Copy, Select All, Preferences |
| View | How the image is displayed; never changes data | Zoom, Fit to Window, Brightness and Contrast, Lookup Table, Channels |
| Image | The image's structure: type, size, axes, geometry | Info, Type, Crop, Scale, Rotate, Duplicate, Stacks, Color |
| Process | Changing sample values | Invert, Filters, Math, Binary, FFT, Subtract Background |
| Analyze | Getting numbers and plots out | Measure, Set Scale, Histogram, Plot Profile, Analyze Particles |
| Extensions | Installed extensions and their manager | |
| Window | Panels, layouts, open images | Reset Layout |
| Help | Command palette, documentation, updates, about | |

- At most two levels: a menu and one submenu.
- Within a menu, related commands are grouped by weight in steps of 10, with
  a separator wherever neighbours fall in different hundreds.
- A menu this table does not name is placed just before Window.
- A command appears in exactly one place.

Differences from ImageJ worth knowing: *Invert* is under Process (it changes
values) rather than Edit; display adjustments are under View rather than
Image > Adjust; *Plugins* is *Extensions*.

## Naming

- Titles use title case and say what the command does: "Gaussian Blur",
  "Save As". No trailing "...": the interface adds an ellipsis itself when
  running the command will open a dialog.
- Summaries are one sentence ending in a full stop, written for someone who
  does not know the command.
- Parameter labels are sentence case without a trailing colon: "Radius",
  "Output type". Units are declared, not typed into the label.
- Command ids are `group.verb_noun` or `group.noun`, grouped by subject
  rather than by menu: `file.open`, `image.invert`, `filter.gaussian_blur`,
  `stack.z_project`, `view.zoom_in`. Ids never change once released.
- Words used consistently: *image* (not picture, dataset or window),
  *plane* (one XY slice), *channel*, *slice* (Z), *frame* (time),
  *selection* (ROI), *lookup table* (LUT in compact places), *extension*
  (not plugin).

## Command palette

Ctrl+K (also Ctrl+Shift+P) opens a search box over the window. It searches
titles, summaries, ids and ImageJ names, ranks prefix and word-start matches
first, and shows each command's menu path and shortcut. Commands that cannot
run now are listed, dimmed, with the reason. Enter runs; Esc closes.

## Dialogs

A command that needs values from the user gets a dialog generated from its
parameters:

- One row per parameter: label, control, unit. Help text appears on hover.
- Controls by type: checkbox (bool), number field with drag and optional
  slider when bounded (int, float), text field (string), drop-down (choice),
  path field with a browse button (path), image drop-down (image).
- Advanced parameters sit in a collapsed "More options" section.
- Previewable commands show a "Preview" checkbox; the preview updates
  shortly after the last change and is shown on the current plane only.
- Buttons: the command's verb or "OK" on the right as the default, "Cancel",
  and "Reset" to restore defaults. Enter accepts, Esc cancels.
- Invalid values are marked on the field with the reason; OK is disabled
  until they are fixed.
- Values are remembered per command for the next use.
- If the only thing to ask for is a file path (the active image aside), the
  native file dialog opens directly instead. The window stays responsive
  while it is open.
- Dialogs ask only for what the interface cannot already answer: commands
  that act on a plane or channel take them from the view.

## Viewer

- Samples are never smoothed when zoomed in; pixels stay sharp.
- A newly opened image is shown at the largest standard zoom step at which
  all of it fits, so a small image is enlarged rather than shown as a speck.
- Mouse wheel zooms about the cursor. Drag with the middle button, or hold
  Space and drag, to pan. Double-click the zoom readout to fit.
- Shortcuts: `+` and `-` zoom, `0` fits the window, `1` shows 100%.
- Sliders under the image step through Z and time; channels have toggle
  buttons. Left and Right (*View > Previous Plane* and *Next Plane*) step
  the last-used slider; Shift+wheel does too.
- Dragging a display-range control updates the image as it moves and records
  one History entry when released.
- The status bar shows the position under the cursor in pixels and in
  calibrated units, and the sample value (all channels for composites).
- An image larger than memory opens immediately and loads planes on demand.

## Feedback

- **Status bar:** the result of the last action on the left.
- **Progress:** commands that take longer than about half a second show a
  progress bar with their name and a Cancel button; the interface stays
  usable.
- **Errors:** shown in the status bar in the error colour and recorded in
  the Log. They state what failed, on what, and why, in plain words. No
  modal error boxes.
- **Unavailable commands** are disabled, with the reason on hover ("Open an
  image first"). Menu items otherwise have no tooltips; summaries are shown
  in the command palette.
- **History:** clicking an entry undoes or redoes to that step.

## Shortcuts

| Action | Shortcut |
|---|---|
| Open, Save, Save As, Close | Ctrl+O, Ctrl+S, Ctrl+Shift+S, Ctrl+W |
| Undo, Redo | Ctrl+Z, Ctrl+Shift+Z |
| Command palette | Ctrl+K |
| Zoom in, out, fit, 100% | `+`, `-`, `0`, `1` |

Ctrl is Cmd on macOS. Shortcuts are declared on commands, shown in menus
and the palette, and never hard-coded in interface code.

## Appearance

- Follows the system light or dark theme; *View > Theme* overrides it.
- One shared set of widgets (in the application's `ui` module) is used for
  every panel and dialog: same spacing, same number fields, same buttons.
- The image area has a neutral mid-dark background in both themes so that
  image brightness is judged consistently.
- Colour is never the only signal: errors also carry text, toggles also
  change shape or label.
