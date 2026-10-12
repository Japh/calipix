// Opens the selection files Calipix wrote with real ImageJ and prints what
// ImageJ makes of them, in exactly the format of the `expected.txt` that the
// ignored test `write_files_for_imagej_check` writes next to them.
//
// Manual verification, not part of `cargo test`:
//
//   CALIPIX_ROI_CHECK_DIR=<dir> cargo test -p calipix-roi --test imagej_files write_files_for_imagej_check -- --ignored
//   java -Duser.home=<empty dir> -jar ij-1.54p.jar -batch imagej_open_check.ijm <dir>  > actual.txt
//   compare actual.txt with <dir>/expected.txt
//
// ImageJ must run without -Djava.awt.headless=true, and a macro error opens
// a dialog and hangs, so run it under a timeout.
function report(name) {
	type = selectionType();
	getSelectionBounds(x, y, w, h);
	area = "-";
	if (type < 5 || type == 9) area = "" + getValue("Area");
	length = "-";
	if (type < 5 || type == 9) length = d2s(getValue("Perim."), 4);
	else if (type == 8) length = d2s(getValue("Perim."), 4);
	else if (type != 10) length = d2s(getValue("Length"), 4);
	print(name + " type=" + type + " bounds=" + x + "," + y + "," + w + "," + h + " area=" + area + " length=" + length);
}

dir = getArgument();
names = newArray("rectangle", "rectangle_sub", "rounded", "oval", "oval_sub", "polygon", "polygon_sub", "polygon_cross",
	"freehand", "traced", "line", "line_sub_wide", "polyline", "polyline_sub_wide", "freeline", "angle", "point",
	"points_sub", "ellipse", "rotated", "composite_hole", "composite_curves");
newImage("host", "8-bit black", 64, 48, 1);
for (i = 0; i < names.length; i++) {
	run("Select None");
	open(dir + "/" + names[i] + ".roi");
	report(names[i]);
}
run("Select None");
roiManager("Open", dir + "/set.zip");
if (roiManager("count") != names.length) print("set.zip holds " + roiManager("count") + " selections");
for (i = 0; i < roiManager("count"); i++) {
	roiManager("select", i);
	report(getInfo("selection.name"));
}
print("CHECK_DONE");
