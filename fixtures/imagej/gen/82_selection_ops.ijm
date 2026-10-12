// Commands run with a selection that is not a rectangle: filters and arithmetic confined to it, the commands
// that act on its bounding rectangle instead, and what the commands that reshape an image do to it
// (depends on 01_planes.ijm).
// Sources:
//   gray8, gray16, gray32f, rgb : the base planes of 01_planes.ijm cropped to the 31x23 rectangle at (16,12)
// Selections, by name (selection_ops.csv has the Calipix text form of each, with ";" for its commas):
//   oval      makeOval(6, 4, 18, 13)
//   polygon   makePolygon(3,2, 27,5, 12,9, 25,20, 2,18)
//   polyf     a triangle with fractional vertices
//   hole      the rectangle (4,3,22,16) less the oval (9,6,10,8): a composite selection
//   rounded   makeRectangle(5, 4, 20, 14, 8)
//   cross     makeOval(20, 12, 18, 16), which crosses the right and bottom edges
//   ellipse   makeEllipse(5, 5, 25, 17, 0.5)
//   rotrect   makeRotatedRectangle(6, 6, 24, 16, 8)
//   line      makeLine(2, 3, 28, 19)
//   points    two points
// Outputs:
//   selop_<bits>_<width>x<height>.raw   result planes with that pixel type and size, in csv order
//   selection_ops.csv   group,slice,count,source,selection,roi,command,options,width,height,bits,
//                       sel_type,sel_x,sel_y,sel_w,sel_h,sel_area,sel_cx,sel_cy,sel_coords
//                       The result is the image that is current after the command. sel_type is its
//                       selectionType() (-1: none), followed by getSelectionBounds; for an area selection
//                       sel_area is the number of pixels it covers and sel_cx, sel_cy their centroid;
//                       sel_coords is getSelectionCoordinates as x:y pairs separated by spaces.
var d = getArgument();
var csv = "group,slice,count,source,selection,roi,command,options,width,height,bits,sel_type,sel_x,sel_y,sel_w,sel_h,sel_area,sel_cx,sel_cy,sel_coords\n";
var roiText = "";
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
run("Colors...", "foreground=black background=white selection=yellow");
run("Set Measurements...", "area centroid redirect=None decimal=9");

areas = newArray("oval", "polygon", "polyf", "hole", "rounded", "cross", "ellipse", "rotrect");
all = Array.concat(areas, newArray("line", "points"));

// Confined to the selection (PlugInFilter.SUPPORTS_MASKING).
masked = newArray("Gaussian Blur...|sigma=2", "Median...|radius=2", "Add...|value=25", "Invert|");
for (i=0; i<areas.length; i++)
	for (j=0; j<masked.length; j++)
		op("gray8", areas[i], masked[j]);
more = newArray("Multiply...|value=1.5", "Unsharp Mask...|radius=1 mask=0.60", "Mean...|radius=1.5", "Smooth|", "Find Edges|",
	"Rotate... |angle=15 grid=1 interpolation=Bilinear", "Rotate... |angle=-30 grid=1 interpolation=None");
two = newArray("oval", "polygon");
for (i=0; i<two.length; i++)
	for (j=0; j<more.length; j++)
		op("gray8", two[i], more[j]);
others = newArray("gray16", "gray32f", "rgb");
some = newArray("Gaussian Blur...|sigma=2", "Median...|radius=2", "Add...|value=25");
for (k=0; k<others.length; k++)
	for (i=0; i<two.length; i++)
		for (j=0; j<some.length; j++)
			op(others[k], two[i], some[j]);

// On the bounding rectangle, whatever the shape (no SUPPORTS_MASKING).
bounds = newArray("Flip Horizontally|", "Flip Vertically|", "Translate...|x=5 y=-3 interpolation=None");
shapes = newArray("oval", "polygon", "polyf", "hole", "cross", "rotrect");
for (i=0; i<shapes.length; i++)
	for (j=0; j<bounds.length; j++)
		op("gray8", shapes[i], bounds[j]);
op("gray8", "oval", "Translate...|x=2.5 y=1.5 interpolation=Bilinear");
for (k=0; k<others.length; k++) {
	op(others[k], "oval", "Flip Horizontally|");
	op(others[k], "polygon", "Translate...|x=5 y=-3 interpolation=None");
}

// A line or points do not confine anything: the whole image is processed.
lines = newArray("line", "points");
whole = newArray("Gaussian Blur...|sigma=2", "Add...|value=25", "Flip Horizontally|", "Translate...|x=5 y=-3 interpolation=None",
	"Rotate... |angle=15 grid=1 interpolation=Bilinear");
for (i=0; i<lines.length; i++)
	for (j=0; j<whole.length; j++)
		op("gray8", lines[i], whole[j]);

// What becomes of the selection when the image is reshaped or converted.
reshape = newArray("Crop|", "Duplicate...|title=copy", "Duplicate...|title=copy ignore", "Rotate 90 Degrees Right|",
	"Rotate 90 Degrees Left|", "Scale...|x=0.5 y=0.5 interpolation=Bilinear average", "Scale...|x=0.5 y=0.5 interpolation=Bilinear average create",
	"Size...|width=20 height=15 average interpolation=Bilinear", "Canvas Size...|width=40 height=30 position=Center zero",
	"16-bit|", "32-bit|", "RGB Color|");
for (i=0; i<all.length; i++) {
	for (j=0; j<reshape.length; j++) {
		// Crop needs an area; ImageJ only reports an error otherwise.
		if (!(startsWith(reshape[j], "Crop") && i>=areas.length))
			op("gray8", all[i], reshape[j]);
	}
}
op("gray16", "oval", "8-bit|");
op("rgb", "polygon", "8-bit|");
op("gray8", "oval", "calculator|add create");
op("gray8", "polygon", "calculator|subtract");

for (i=nImages; i>=1; i--) {
	selectImage(i);
	t = getTitle();
	if (startsWith(t, "selop_")) {
		saveAs("Raw Data", d+"/"+t+".raw");
		close();
	}
}
run("Set Measurements...", "area mean min redirect=None decimal=3");
File.saveString(csv, d+"/selection_ops.csv");
print("GEN_OK");

function source(name) {
	open(d+"/"+name+".tif");
	makeRectangle(16, 12, 31, 23);
	run("Crop");
	run("Select None");
}

function select(name) {
	if (name=="oval") {
		makeOval(6, 4, 18, 13); roiText = "oval 6 4 18 13";
	} else if (name=="polygon") {
		makePolygon(3, 2, 27, 5, 12, 9, 25, 20, 2, 18); roiText = "polygon 3;2 27;5 12;9 25;20 2;18";
	} else if (name=="polyf") {
		makeSelection("polygon", newArray(3.5, 27.25, 12.75), newArray(2.25, 5.5, 20.4)); roiText = "polygon 3.5;2.25 27.25;5.5 12.75;20.4";
	} else if (name=="hole") {
		makeRectangle(4, 3, 22, 16); setKeyDown("alt"); makeOval(9, 6, 10, 8); setKeyDown("none");
		roiText = "rectangle 4 3 22 16 minus oval 9 6 10 8";
	} else if (name=="rounded") {
		makeRectangle(5, 4, 20, 14, 8); roiText = "rounded-rectangle 5 4 20 14 8";
	} else if (name=="cross") {
		makeOval(20, 12, 18, 16); roiText = "oval 20 12 18 16";
	} else if (name=="ellipse") {
		makeEllipse(5, 5, 25, 17, 0.5); roiText = "ellipse 5 5 25 17 0.5";
	} else if (name=="rotrect") {
		makeRotatedRectangle(6, 6, 24, 16, 8); roiText = "rotated-rectangle 6 6 24 16 8";
	} else if (name=="line") {
		makeLine(2, 3, 28, 19); roiText = "line 2 3 28 19";
	} else if (name=="points") {
		makeSelection("point", newArray(4, 20), newArray(5, 15)); roiText = "points 4;5 20;15";
	} else
		exit("unknown selection "+name);
}

function op(name, selection, what) {
	parts = split(what, "|");
	command = parts[0];
	options = "";
	if (parts.length>1) options = parts[1];
	print("selop "+name+" ["+selection+"] "+command+" "+options);
	source(name);
	select(selection);
	if (command=="calculator") {
		// The second operand: the same picture flipped.
		id1 = getImageID();
		run("Duplicate...", "title=second ignore");
		run("Select None");
		run("Flip Horizontally");
		selectImage(id1);
		words = split(options, " ");
		arg = words[0];
		if (words.length>1) arg = arg + " " + words[1];
		imageCalculator(arg, id1, "second");
		if (words.length==1) selectImage(id1);
	} else
		run(command, options);
	record(name, selection, command, options);
	for (i2=nImages; i2>=1; i2--) {
		selectImage(i2);
		if (!startsWith(getTitle(), "selop_")) close();
	}
}

// Appends the current image to its group and adds its row to the table.
function record(name, selection, command, options) {
	bd = bitDepth();
	w = getWidth(); h = getHeight();
	count = nSlices;
	st = selectionType();
	sx = 0; sy = 0; sw = 0; sh = 0;
	area = "NaN"; cx = "NaN"; cy = "NaN"; coords = "";
	if (st>=0) {
		getSelectionBounds(sx, sy, sw, sh);
		getSelectionCoordinates(xs, ys);
		for (k2=0; k2<xs.length; k2++) {
			if (k2>0) coords = coords + " ";
			coords = coords + d2s(xs[k2], 4) + ":" + d2s(ys[k2], 4);
		}
		if (st<=4 || st==9) {
			List.setMeasurements;
			area = List.getValue("Area");
			cx = d2s(List.getValue("X"), 6);
			cy = d2s(List.getValue("Y"), 6);
		}
	}
	group = "selop_" + bd + "_" + w + "x" + h;
	first = collect(group);
	csv = csv + group + "," + first + "," + count + "," + name + "," + selection + "," + roiText + "," + command + "," + options + "," + w + "," + h + "," + bd
		+ "," + st + "," + sx + "," + sy + "," + sw + "," + sh + "," + area + "," + cx + "," + cy + "," + coords + "\n";
}

// Appends every plane of the current image to the stack called 'group' and returns the slice
// number of the first one there.
function collect(group) {
	id = getImageID();
	w = getWidth(); h = getHeight(); bd = bitDepth(); count = nSlices;
	first = 0;
	a = newArray(w*h);
	for (s=1; s<=count; s++) {
		selectImage(id);
		setSlice(s);
		for (y=0; y<h; y++) for (x=0; x<w; x++) a[y*w+x] = getPixel(x, y);
		if (isOpen(group)) {
			selectImage(group);
			setSlice(nSlices);
			run("Add Slice");
		} else {
			type = "" + bd + "-bit black";
			if (bd==24) type = "RGB black";
			newImage(group, type, w, h, 1);
		}
		for (y=0; y<h; y++) for (x=0; x<w; x++) setPixel(x, y, a[y*w+x]);
		if (s==1) first = nSlices;
	}
	selectImage(id);
	return first;
}
