// Transforms that interpolate: Rotate, Scale, Size (and their stack and Z forms), and Translate by
// a fraction of a pixel.
// Sources:
//   gray8, gray16, gray32f, rgb : the base planes of 01_planes.ijm cropped to the 31x23 rectangle
//                                 at (16,12); gray32f then gets NaN at (2,1)
//   row    : gray8 cropped to the 31x1 rectangle at (16,12)
//   stack  : stack_z6_8bit.tif (32x24, 6 slices)
//   hyper  : channels 1-3, slices 1-2, frames 2-3 of hyperstack_c3z4t5.tif (12 planes, composite)
// Setup words, applied in order before the command:
//   sel=<x>:<y>:<w>:<h>  makeRectangle     slice=<n>  setSlice     range=<min>:<max>  setMinAndMax
//   cal   pixel size 0.5 x 0.25 x 2 um with the origin at pixel (3,5)
//   pos=<c>:<z>:<t>      Stack.setPosition
// Outputs:
//   rs_<bits>_<width>x<height>.raw   result planes with that pixel type and size, in csv order
//   rotate_scale.csv   the columns of geometry.csv (see 63_geometry.ijm)
var d = getArgument();
var csv = "group,slice,count,source,setup,command,options,width,height,bits,channels,slices,frames,min,max,pixel_width,pixel_height,pixel_depth,unit,x_origin,y_origin,selection,sel_x,sel_y,sel_w,sel_h
";
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
run("Colors...", "foreground=black background=white selection=yellow");

types = newArray("gray8", "gray16", "gray32f", "rgb");
methods = newArray("None", "Bilinear", "Bicubic");
for (i=0; i<types.length; i++) {
	s = types[i];
	for (j=0; j<methods.length; j++) {
		m = " interpolation=" + methods[j];
		o = "angle=15 grid=1" + m;
		geo(s, "-", "Rotate... ", o);
		o = "angle=-37.5 grid=1" + m;
		geo(s, "sel=4:3:20:15", "Rotate... ", o);
		o = "x=2.5 y=1.25" + m;
		geo(s, "-", "Translate...", o);
		o = "x=0.5 y=0.5" + m + " average create";
		geo(s, "-", "Scale...", o);
		o = "x=0.4 y=0.6" + m + " create";
		geo(s, "-", "Scale...", o);
		o = "x=1.7 y=2.3" + m + " average create";
		geo(s, "-", "Scale...", o);
		o = "x=0.5 y=1.5" + m + " average create";
		geo(s, "-", "Scale...", o);
		o = "x=0.6 y=0.6" + m;
		geo(s, "-", "Scale...", o);
		o = "x=1.5 y=1.5" + m;
		geo(s, "sel=8:6:12:9", "Scale...", o);
		o = "width=20 height=10 average" + m;
		geo(s, "-", "Size...", o);
		o = "width=50 height=40" + m;
		geo(s, "-", "Size...", o);
	}
}
s = "gray8";
geo(s, "-", "Rotate... ", "angle=90 grid=1 interpolation=Bilinear");
geo(s, "-", "Rotate... ", "angle=0 grid=1 interpolation=Bilinear");
geo(s, "-", "Rotate... ", "angle=180 grid=1 interpolation=None");
geo(s, "-", "Rotate... ", "angle=15 grid=1 interpolation=Bilinear enlarge");
geo(s, "-", "Rotate... ", "angle=-60 grid=1 interpolation=None enlarge");
geo(s, "cal", "Rotate... ", "angle=15 grid=1 interpolation=Bilinear enlarge");
geo("gray32f", "-", "Rotate... ", "angle=0 grid=1 interpolation=Bilinear");
geo("gray32f", "-", "Rotate... ", "angle=33 grid=1 interpolation=Bicubic enlarge");
geo("gray16", "-", "Rotate... ", "angle=33 grid=1 interpolation=Bilinear enlarge");
geo("rgb", "-", "Rotate... ", "angle=33 grid=1 interpolation=Bilinear enlarge");
geo(s, "-", "Rotate... ", "angle=15 grid=1");
geo(s, "sel=4:3:20:15", "Scale...", "x=0.5 y=0.5 interpolation=Bilinear average create");
geo(s, "sel=4:3:20:15", "Scale...", "x=2 y=2 interpolation=Bicubic average create");
geo(s, "-", "Scale...", "x=- y=- width=13 height=40 interpolation=Bilinear average create");
geo(s, "cal", "Scale...", "x=0.5 y=0.25 interpolation=Bilinear average create");
geo(s, "cal", "Size...", "width=20 height=10 average interpolation=Bilinear");
geo(s, "-", "Size...", "width=20 height=10 constrain average interpolation=Bilinear");
geo(s, "sel=4:3:20:15", "Size...", "width=30 height=30 average interpolation=Bilinear");
geo(s, "-", "Size...", "width=20 height=10 interpolation=Bilinear");
geo(s, "-", "Scale...", "x=0.3 y=0.3 interpolation=None average create");
geo(s, "-", "Scale...", "x=0.3 y=0.3 interpolation=Bicubic average create");
geo("gray16", "range=1000:50000", "Scale...", "x=0.6 y=0.6 interpolation=Bilinear");
geo("gray32f", "range=-10:20", "Scale...", "x=0.6 y=0.6 interpolation=None");
geo("row", "-", "Size...", "width=50 height=3 interpolation=Bilinear");
geo("row", "-", "Size...", "width=20 height=1 average interpolation=Bilinear");
geo("row", "-", "Scale...", "x=2 y=3 interpolation=Bicubic create");

s = "stack";
geo(s, "slice=2", "Rotate... ", "angle=15 grid=1 interpolation=Bilinear stack");
geo(s, "slice=2", "Rotate... ", "angle=15 grid=1 interpolation=Bilinear slice");
geo(s, "slice=2", "Rotate... ", "angle=15 grid=1 interpolation=Bilinear enlarge stack");
geo(s, "slice=2", "Scale...", "x=0.5 y=0.5 z=1.0 interpolation=Bilinear average process create");
geo(s, "slice=2", "Scale...", "x=0.5 y=0.5 z=1.0 interpolation=Bilinear average create");
geo(s, "slice=2", "Scale...", "x=0.5 y=0.5 z=1.0 interpolation=Bilinear process");
geo(s, "slice=2", "Scale...", "x=0.5 y=0.5 z=1.0 interpolation=Bilinear");
geo(s, "-", "Scale...", "x=1.0 y=1.0 z=0.5 interpolation=Bilinear average process create");
geo(s, "-", "Scale...", "x=1.0 y=1.0 z=1.5 interpolation=Bilinear average process create");
geo(s, "-", "Scale...", "x=0.5 y=0.5 z=0.5 interpolation=Bicubic average process create");
geo(s, "-", "Scale...", "x=1.0 y=1.0 z=0.5 interpolation=None process create");
geo(s, "-", "Size...", "width=20 height=10 depth=6 average interpolation=Bilinear");
geo(s, "-", "Size...", "width=32 height=24 depth=3 average interpolation=Bilinear");
geo(s, "-", "Size...", "width=32 height=24 depth=9 average interpolation=Bicubic");
geo(s, "slice=2", "Translate...", "x=2.5 y=1.25 interpolation=Bilinear stack");

s = "hyper";
geo(s, "-", "Scale...", "x=0.5 y=0.5 z=1.0 interpolation=Bilinear average create");
geo(s, "pos=2:2:1", "Rotate... ", "angle=15 grid=1 interpolation=Bilinear slice");
geo(s, "-", "Size...", "width=20 height=10 depth=2 time=2 average interpolation=Bilinear");

for (i=nImages; i>=1; i--) {
	selectImage(i);
	title = getTitle();
	saveAs("Raw Data", d+"/"+title+".raw");
}
File.saveString(csv, d+"/rotate_scale.csv");
print("GEN_OK");

function source(name) {
	if (name=="stack") {
		open(d+"/stack_z6_8bit.tif");
	} else if (name=="hyper") {
		open(d+"/hyperstack_c3z4t5.tif");
		run("Duplicate...", "title=hyper duplicate channels=1-3 slices=1-2 frames=2-3");
	} else if (name=="row") {
		open(d+"/gray8.tif");
		makeRectangle(16, 12, 31, 1);
		run("Crop");
	} else {
		open(d+"/"+name+".tif");
		makeRectangle(16, 12, 31, 23);
		run("Crop");
		if (name=="gray32f") setPixel(2, 1, NaN);
	}
}

function geo(name, setup, command, options) {
	print("geo "+name+" ["+setup+"] "+command+" "+options);
	source(name);
	if (setup!="-") {
		p = split(setup, " ");
		for (i=0; i<p.length; i++) {
			if (startsWith(p[i], "sel=")) {
				r = split(substring(p[i], 4), ":");
				makeRectangle(parseInt(r[0]), parseInt(r[1]), parseInt(r[2]), parseInt(r[3]));
			} else if (startsWith(p[i], "range=")) {
				r = split(substring(p[i], 6), ":");
				setMinAndMax(parseFloat(r[0]), parseFloat(r[1]));
			} else if (startsWith(p[i], "slice="))
				setSlice(parseInt(substring(p[i], 6)));
			else if (startsWith(p[i], "pos=")) {
				r = split(substring(p[i], 4), ":");
				Stack.setPosition(parseInt(r[0]), parseInt(r[1]), parseInt(r[2]));
			} else if (p[i]=="cal")
				run("Properties...", "unit=um pixel_width=0.5 pixel_height=0.25 voxel_depth=2 origin=3,5");
		}
	}
	run(command, options);
	record(name, setup, command, options);
	for (i=nImages; i>=1; i--) {
		selectImage(i);
		if (!startsWith(getTitle(), "rs_")) close();
	}
}

// Appends the current image to its group and adds its row to the table.
function record(name, setup, command, options) {
	bd = bitDepth();
	Stack.getDimensions(w, h, c, z, t);
	count = nSlices;
	getMinAndMax(mn, mx);
	getVoxelSize(pw, ph, pd, unit);
	xo = 0; yo = 0;
	toUnscaled(xo, yo);
	st = selectionType();
	getSelectionBounds(sx, sy, sw, sh);
	group = "rs_" + bd + "_" + w + "x" + h;
	first = collect(group);
	csv = csv + group + "," + first + "," + count + "," + name + "," + setup + "," + command + "," + options + "," + w + "," + h + "," + bd + "," + c + "," + z + "," + t + "," + mn + "," + mx + "," + pw + "," + ph + "," + pd + "," + unit + "," + xo + "," + yo + "," + st + "," + sx + "," + sy + "," + sw + "," + sh + "\n";
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
