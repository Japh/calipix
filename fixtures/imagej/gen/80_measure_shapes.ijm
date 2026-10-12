// Analyze>Measure on every kind of selection (depends on 01_planes.ijm and 40_roi_shapes.ijm).
// Each selection is one of the roi_<case>.roi files, put on a 64x48 image, and measured with
// run("Measure") into a cleared Results table.
// Output:
//   measure_shape_cases.txt  one block per measurement, in the layout of measure_cases.txt:
//       # name
//       @image=FILE           the image measured
//       @cal=PW PH UNIT XO YO pixel size, unit and origin in pixels set with Image>Properties (absent: none)
//       @invert_y=1           "Invert Y coordinates" was set in Analyze>Set Measurements
//       @measurements=WORDS   the Set Measurements keywords (absent: every measurement)
//       @roi_file=FILE        the selection, read from a file. ImageJ reads the file of a rotated rectangle
//                             back as a plain polygon with the same four vertices.
//       @roi=TEXT             the selection, made by the macro: "rotated-rectangle x1 y1 x2 y2 width", or
//                             "line x1 y1 x2 y2 width w" followed by @to_area=1 for Edit>Selection>Line to Area
//       @type=N               selectionType() once the selection was on the image
//       @points=N, @row=K     point selections of several points: Measure adds a row per point, and the block
//                             holds row K (from 0). ImageJ run without a window keeps only the last row, which
//                             is the block named after the case; the blocks named case.K are that point
//                             measured as a selection of its own, which is how Analyzer.measurePoint measures it.
//       Heading=V             every column of the row Measure added, as Java prints a double
var d = "";
var s = "";
var all = "area mean standard modal min centroid center perimeter bounding fit shape feret's integrated median skewness kurtosis area_fraction stack";
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
run("Set Measurements...", all + " redirect=None decimal=9");

list = getFileList(d);
Array.sort(list);
rois = newArray(0);
for (i=0; i<list.length; i++) {
	if (startsWith(list[i], "roi_") && endsWith(list[i], ".roi"))
		rois = Array.concat(rois, list[i]);
}

// Every selection on an uncalibrated 8-bit image.
open(d+"/gray8.tif");
for (i=0; i<rois.length; i++)
	measureCase("plain", "@image=gray8.tif\n", rois[i]);
madeCases("plain", "@image=gray8.tif\n");

// Pixels that are not square, with a unit and an origin.
run("Properties...", "channels=1 slices=1 frames=1 pixel_width=0.5 pixel_height=0.25 voxel_depth=1 unit=mm origin=3,4");
for (i=0; i<rois.length; i++)
	measureCase("cal", "@image=gray8.tif\n@cal=0.5 0.25 mm 3 4\n", rois[i]);
madeCases("cal", "@image=gray8.tif\n@cal=0.5 0.25 mm 3 4\n");

// Square pixels of a fractional size: ellipses and rotated rectangles then use their own axes.
some = newArray("ellipse", "ellipse_sub", "ellipse_circle", "rotrect", "rotrect_sub", "rotate_rotrect_20", "line_wide",
	"line_sub", "polyline_int", "polyline_wide", "freeline_sub", "angle_sub", "points_sub", "points_cross", "comp_hole", "rounded", "oval_sub");
run("Properties...", "channels=1 slices=1 frames=1 pixel_width=0.3 pixel_height=0.3 voxel_depth=1 unit=um origin=0,0");
for (i=0; i<some.length; i++)
	measureCase("square", "@image=gray8.tif\n@cal=0.3 0.3 um 0 0\n", "roi_"+some[i]+".roi");
madeCases("square", "@image=gray8.tif\n@cal=0.3 0.3 um 0 0\n");

// Invert Y, with an origin.
run("Properties...", "channels=1 slices=1 frames=1 pixel_width=0.5 pixel_height=0.25 voxel_depth=1 unit=mm origin=3,4");
run("Set Measurements...", all + " invert redirect=None decimal=9");
for (i=0; i<some.length; i++)
	measureCase("inverty", "@image=gray8.tif\n@cal=0.5 0.25 mm 3 4\n@invert_y=1\n", "roi_"+some[i]+".roi");
run("Set Measurements...", all + " redirect=None decimal=9");

// Fewer measurements: lines then give only their length, angles and points their own columns.
few = newArray("perimeter", "area", "mean bounding", "centroid feret's", "shape", "fit", "stack");
for (k=0; k<few.length; k++) {
	run("Set Measurements...", few[k] + " redirect=None decimal=9");
	for (i=0; i<some.length; i++)
		measureCase("few"+k, "@image=gray8.tif\n@cal=0.5 0.25 mm 3 4\n@measurements="+few[k]+"\n", "roi_"+some[i]+".roi");
}
run("Set Measurements...", all + " redirect=None decimal=9");
close();

// Other pixel types.
images = newArray("gray16.tif", "gray32f.tif", "rgb.tif");
for (k=0; k<images.length; k++) {
	open(d+"/"+images[k]);
	base = replace(images[k], ".tif", "");
	for (i=0; i<some.length; i++)
		measureCase(base, "@image="+images[k]+"\n", "roi_"+some[i]+".roi");
	close();
}

run("Set Measurements...", "area mean min redirect=None decimal=3");
File.saveString(s, d+"/measure_shape_cases.txt");
print("GEN_OK");

function measureCase(prefix, head, file) {
	run("Select None");
	// Through the ROI Manager: File>Open would put a selection that crosses the image edge on a new image.
	roiManager("reset");
	roiManager("Open", d+"/"+file);
	roiManager("select", 0);
	type = selectionType();
	if (type<0) return;
	name = replace(replace(file, "roi_", ""), ".roi", "");
	head = head + "@roi_file="+file+"\n@type="+type+"\n";
	points = 1;
	if (type==10) points = getValue("selection.size");
	if (points>1) {
		// Without a Results window every point restarts the table, so the last point's row is what remains.
		// The rows are then measured one point at a time, which is what Analyzer.measurePoint does.
		getSelectionCoordinates(px, py);
		head = head + "@points="+points+"\n";
		s = s + "# "+prefix+"_"+name+"\n" + head + "@row="+(points-1)+"\n" + measured();
		for (k2=0; k2<points; k2++) {
			makePoint(px[k2], py[k2]);
			s = s + "# "+prefix+"_"+name+"."+k2+"\n" + head + "@row="+k2+"\n" + measured();
		}
	} else
		s = s + "# "+prefix+"_"+name+"\n" + head + measured();
	run("Select None");
}

// Selections that do not survive being saved: rotated rectangles, made as 40_roi_shapes.ijm makes them, and
// the rotated rectangle Line to Area turns a straight line into.
function madeCases(prefix, head) {
	rr = newArray("10 10 50 35 12", "20.5 40 40 5.5 7.3", "10 24 50 24 10", "-10 10 40 60 15");
	for (k3=0; k3<rr.length; k3++) {
		a = split(rr[k3], " ");
		makeRotatedRectangle(parseFloat(a[0]), parseFloat(a[1]), parseFloat(a[2]), parseFloat(a[3]), parseFloat(a[4]));
		s = s + "# "+prefix+"_made_rotrect"+k3+"\n" + head + "@roi=rotated-rectangle "+rr[k3]+"\n@type="+selectionType()+"\n" + measured();
	}
	lines = newArray("5 5 50 30 7", "2 40 60 44 1");
	for (k3=0; k3<lines.length; k3++) {
		a = split(lines[k3], " ");
		makeLine(parseFloat(a[0]), parseFloat(a[1]), parseFloat(a[2]), parseFloat(a[3]), parseFloat(a[4]));
		run("Line to Area");
		s = s + "# "+prefix+"_made_line_area"+k3+"\n" + head + "@roi=line "+a[0]+" "+a[1]+" "+a[2]+" "+a[3]+" width "+a[4]
			+ "\n@to_area=1\n@type="+selectionType()+"\n" + measured();
	}
	run("Select None");
	run("Line Width...", "line=1");
}

function measured() {
	run("Clear Results");
	run("Measure");
	text = "";
	headings = split(String.getResultsHeadings);
	for (h=0; h<headings.length; h++)
		text = text + headings[h] + "=" + String.format("%.17g", getResult(headings[h], nResults-1)) + "\n";
	return text + "\n";
}
