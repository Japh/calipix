// Full-precision references for Analyze>Measure (depends on 01_planes.ijm, 03_hyperstack.ijm,
// 04_luts_calibration.ijm and 06_measure.ijm).
// Outputs:
//   measure_cases.txt        one block per measurement. A block starts with "# name"; lines starting with "@"
//                            describe the case, the rest are heading=value with the value as Java prints a double.
//       @image=FILE           the image measured (in this directory)
//       @position=C Z T       hyperstacks only: the plane on view, from 1
//       @cal=PW PH UNIT XO YO pixel size, unit and origin in pixels set with Image>Properties (absent: as stored)
//       @invert_y=1           "Invert Y coordinates" was set in Analyze>Set Measurements
//       @roi=KIND ARGS        none | rect x y w h | oval x y w h | polygon x y ... | freehand x y ... |
//                             line x1 y1 x2 y2 | point x y
//       @bounds=X Y W H       getSelectionBounds
//       @coords=X,Y ...       getSelectionCoordinates (the outline of the selection as a float polygon)
//       @mask=FILE            the pixels the selection covers (Edit>Selection>Create Mask), 255 inside
//     Area selections and "none" are measured with List.setMeasurements (every measurement on, no rounding).
//     Lines and points are measured with getValue(heading) for every heading, which runs Analyze>Measure on a
//     private table; a heading the measurement does not produce gives NaN.
//   measure_mask_<shape>.png  the masks named by @mask
var d = "";
var s = "";
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
run("Set Measurements...", "area mean min redirect=None decimal=3");
s = "";

images = newArray("gray8.tif", "gray16.tif", "gray32f.tif", "rgb.tif", "blobs.tif");
for (i=0; i<images.length; i++) {
	open(d+"/"+images[i]);
	base = replace(images[i], ".tif", "");
	saveMasks = 0; if (i==0) saveMasks = 1;
	areaCases(base, "@image="+images[i]+"\n", saveMasks);
	lineCases(base, "@image="+images[i]+"\n");
	close();
}

// Pixels that are not square, a unit and an origin (the density calibration of gray8_calibrated.tif is removed:
// only the spatial calibration is under test).
open(d+"/gray8_calibrated.tif");
run("Calibrate...", "function=None");
head = "@image=gray8_calibrated.tif\n";
areaCases("cal", head, 0);
lineCases("cal", head);
close();

// The same pixels with a calibration set here: no origin, and square pixels with a fractional size.
open(d+"/gray8.tif");
run("Properties...", "channels=1 slices=1 frames=1 pixel_width=0.5 pixel_height=0.25 voxel_depth=2 unit=mm origin=0,0");
areaCases("cal_noorigin", "@image=gray8.tif\n@cal=0.5 0.25 mm 0 0\n", 0);
run("Properties...", "channels=1 slices=1 frames=1 pixel_width=0.3 pixel_height=0.3 voxel_depth=1 unit=um origin=0,0");
areaCases("cal_square", "@image=gray8.tif\n@cal=0.3 0.3 um 0 0\n", 0);
lineCases("cal_square", "@image=gray8.tif\n@cal=0.3 0.3 um 0 0\n");
close();

// Invert Y coordinates, without and with an origin.
run("Set Measurements...", "area mean min invert redirect=None decimal=3");
open(d+"/gray8.tif");
areaCases("inverty", "@image=gray8.tif\n@invert_y=1\n", 0);
lineCases("inverty", "@image=gray8.tif\n@invert_y=1\n");
close();
open(d+"/gray8_calibrated.tif");
run("Calibrate...", "function=None");
areaCases("inverty_cal", "@image=gray8_calibrated.tif\n@invert_y=1\n", 0);
close();
run("Set Measurements...", "area mean min redirect=None decimal=3");

// A plane of a hyperstack: Ch, Slice and Frame.
open(d+"/hyperstack_c3z4t5.tif");
Stack.setPosition(2, 3, 4);
head = "@image=hyperstack_c3z4t5.tif\n@position=2 3 4\n";
run("Select None");
s = s + "# hyper_none\n" + head + "@roi=none\n" + areaValues();
makeRectangle(3, 2, 10, 7);
s = s + "# hyper_rect\n" + head + "@roi=rect 3 2 10 7\n" + areaValues();
makeLine(1, 2, 14, 9);
s = s + "# hyper_line\n" + head + "@roi=line 1 2 14 9\n" + measuredValues() + "\n";
makePoint(4, 5);
s = s + "# hyper_point\n" + head + "@roi=point 4 5\n" + measuredValues() + "\n";
Stack.setPosition(3, 1, 5);
head = "@image=hyperstack_c3z4t5.tif\n@position=3 1 5\n";
makeOval(2, 1, 12, 9);
getSelectionBounds(x, y, w, h);
s = s + "# hyper_oval\n" + head + "@roi=oval 2 1 12 9\n@bounds="+x+" "+y+" "+w+" "+h+"\n@mask=measure_mask_hyper_oval.png\n"
	+ coords() + areaValues();
run("Create Mask");
saveAs("PNG", d+"/measure_mask_hyper_oval.png");
close();
selectImage("hyperstack_c3z4t5.tif");
close();

File.saveString(s, d+"/measure_cases.txt");
print("GEN_OK");

// Measures the active image with no selection and with each area selection.
function areaCases(base, head, saveMasks) {
	run("Select None");
	s = s + "# "+base+"_none\n" + head + "@roi=none\n" + areaValues();
	rects = newArray("8 6 20 10", "0 0 64 48", "44 30 20 18", "0 0 1 1", "63 47 1 1", "10 5 1 7", "5 9 13 1", "0 20 64 3");
	for (r=0; r<rects.length; r++) {
		a = split(rects[r], " ");
		makeRectangle(parseInt(a[0]), parseInt(a[1]), parseInt(a[2]), parseInt(a[3]));
		s = s + "# "+base+"_rect"+r+"\n" + head + "@roi=rect "+rects[r]+"\n" + areaValues();
	}
	ovals = newArray("10 8 30 20", "0 0 64 48", "20 10 9 9", "3 4 5 12", "40 2 24 46", "30 20 2 2");
	for (r=0; r<ovals.length; r++) {
		a = split(ovals[r], " ");
		makeOval(parseInt(a[0]), parseInt(a[1]), parseInt(a[2]), parseInt(a[3]));
		shape("oval"+r, "oval "+ovals[r], base, head, saveMasks);
	}
	polys = newArray("10 5 50 10 40 40 20 30", "5 5 30 8 12 14 28 40 4 44", "0 0 64 0 64 48", "20 20 26 20 26 23 20 23", "33 3 60 20 34 45 45 21");
	for (r=0; r<polys.length; r++) {
		a = split(polys[r], " ");
		xs = newArray(a.length/2); ys = newArray(a.length/2);
		for (k=0; k<xs.length; k++) { xs[k] = parseInt(a[2*k]); ys[k] = parseInt(a[2*k+1]); }
		makeSelection("polygon", xs, ys);
		shape("polygon"+r, "polygon "+polys[r], base, head, saveMasks);
	}
	// A hand-drawn-looking outline: a wobbly closed curve through 40 points.
	n = 40; xs = newArray(n); ys = newArray(n); text = "";
	for (k=0; k<n; k++) {
		t = 2*PI*k/n;
		rad = 14 + 4*sin(3*t) + 2*cos(7*t);
		xs[k] = round(30 + 1.4*rad*cos(t)); ys[k] = round(24 + rad*sin(t));
		text = text + " " + xs[k] + " " + ys[k];
	}
	makeSelection("freehand", xs, ys);
	shape("freehand0", "freehand"+text, base, head, saveMasks);
}

function shape(name, roi, base, head, saveMask) {
	getSelectionBounds(x, y, w, h);
	s = s + "# "+base+"_"+name+"\n" + head + "@roi="+roi+"\n@bounds="+x+" "+y+" "+w+" "+h+"\n@mask=measure_mask_"+name+".png\n"
		+ coords() + areaValues();
	if (saveMask==1) {
		id = getImageID();
		run("Create Mask");
		saveAs("PNG", d+"/measure_mask_"+name+".png");
		close();
		selectImage(id);
	}
}

function coords() {
	getSelectionCoordinates(cx, cy);
	line = "@coords=";
	for (k=0; k<cx.length; k++) {
		if (k>0) line = line + " ";
		line = line + d2s(cx[k], 9) + "," + d2s(cy[k], 9);
	}
	return line + "\n";
}

function areaValues() {
	List.setMeasurements;
	return List.getList + "\n";
}

// Lines and points on the active image.
function lineCases(base, head) {
	lines = newArray("5 6 50 40", "2 10 61 10", "30 2 30 45", "50 40 5 6", "0 0 63 47", "10.5 7.25 40.75 30.5", "7 7 7 7");
	for (r=0; r<lines.length; r++) {
		a = split(lines[r], " ");
		makeLine(parseFloat(a[0]), parseFloat(a[1]), parseFloat(a[2]), parseFloat(a[3]));
		s = s + "# "+base+"_line"+r+"\n" + head + "@roi=line "+lines[r]+"\n" + measuredValues() + "\n";
	}
	points = newArray("12 20", "0 0", "63 47", "31 5");
	for (r=0; r<points.length; r++) {
		a = split(points[r], " ");
		makePoint(parseInt(a[0]), parseInt(a[1]));
		s = s + "# "+base+"_point"+r+"\n" + head + "@roi=point "+points[r]+"\n" + measuredValues() + "\n";
	}
	run("Select None");
}

function measuredValues() {
	keys = newArray("Area","Mean","StdDev","Mode","Min","Max","X","Y","XM","YM","Perim.","BX","BY","Width","Height",
		"Major","Minor","Angle","Circ.","Feret","IntDen","Median","Skew","Kurt","%Area","RawIntDen","Ch","Slice","Frame",
		"FeretX","FeretY","FeretAngle","MinFeret","AR","Round","Solidity","Length");
	text = "";
	for (k=0; k<keys.length; k++) {
		v = getValue(keys[k]);
		text = text + keys[k] + "=" + String.format("%.17g", v) + "\n";
	}
	return text;
}
