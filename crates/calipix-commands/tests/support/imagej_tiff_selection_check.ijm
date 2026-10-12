// Opens the TIFF files Calipix saved with a selection and an overlay in real
// ImageJ and prints what ImageJ shows, in the format of the `expected.txt`
// that the ignored test `write_files_for_imagej_check` of
// `tests/file_selections.rs` writes next to them. Lines starting with "info"
// add the colour and width ImageJ gives each overlay element; they are not
// in expected.txt.
//
// Manual verification, not part of `cargo test`:
//
//   CALIPIX_FILE_CHECK_DIR=<dir> cargo test -p calipix-commands --test file_selections write_files_for_imagej_check -- --ignored
//   java -Duser.home=<empty dir> -jar ij-1.54p.jar -batch imagej_tiff_selection_check.ijm <dir>  > actual.txt
//   compare actual.txt, without its info lines, with <dir>/expected.txt
//
// ImageJ must run without -Djava.awt.headless=true, and a macro error opens
// a dialog and hangs, so run it under a timeout.
function bounds() {
	getSelectionBounds(x, y, w, h);
	return "" + x + "," + y + "," + w + "," + h;
}

function report(name) {
	open(dir + "/" + name + ".tif");
	type = selectionType();
	line = name + " " + getWidth() + "x" + getHeight() + " selection=" + type;
	if (type >= 0) line = line + " bounds=" + bounds();
	n = Overlay.size;
	print(line + " overlay=" + n);
	for (i = 0; i < n; i++) {
		Overlay.activateSelection(i);
		print(name + " overlay " + i + " type=" + selectionType() + " bounds=" + bounds());
		print("info " + name + " overlay " + i + " color=" + Roi.getStrokeColor + " width=" + getValue("selection.width"));
	}
	close();
}

dir = getArgument();
names = newArray("made_here", "imagej_resaved", "changed", "plain");
for (k = 0; k < names.length; k++)
	report(names[k]);
print("CHECK_DONE");
