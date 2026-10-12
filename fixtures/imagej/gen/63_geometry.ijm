// Transforms that only move samples: flips, quarter turns, Flip Z, Crop, Duplicate, Canvas Size
// and Translate.
// Sources:
//   gray8, gray16, gray32f, rgb : the base planes of 01_planes.ijm cropped to the 31x23 rectangle
//                                 at (16,12); gray32f then gets NaN at (2,1)
//   stack  : stack_z6_8bit.tif (32x24, 6 slices)
//   hyper  : channels 1-3, slices 1-2, frames 2-3 of hyperstack_c3z4t5.tif (12 planes, composite)
// Setup words, applied in order before the command:
//   sel=<x>:<y>:<w>:<h>  makeRectangle     slice=<n>  setSlice     range=<min>:<max>  setMinAndMax
//   cal   pixel size 0.5 x 0.25 x 2 um with the origin at pixel (3,5)
//   pos=<c>:<z>:<t>      Stack.setPosition
// Outputs:
//   geo_<bits>_<width>x<height>.raw   result planes with that pixel type and size, in csv order
//   geometry.csv   group,slice,count,source,setup,command,options,width,height,bits,channels,slices,
//                  frames,min,max,pixel_width,pixel_height,pixel_depth,unit,x_origin,y_origin,
//                  selection,sel_x,sel_y,sel_w,sel_h
//                  The result is the image that is current after the command. selection is
//                  selectionType() (-1: none).
var d = getArgument();
var csv = "group,slice,count,source,setup,command,options,width,height,bits,channels,slices,frames,min,max,pixel_width,pixel_height,pixel_depth,unit,x_origin,y_origin,selection,sel_x,sel_y,sel_w,sel_h\n";
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
run("Colors...", "foreground=black background=white selection=yellow");

types = newArray("gray8", "gray16", "gray32f", "rgb");
for (i=0; i<types.length; i++) {
	s = types[i];
	geo(s, "-", "Flip Horizontally", "");
	geo(s, "-", "Flip Vertically", "");
	geo(s, "-", "Rotate 90 Degrees Right", "");
	geo(s, "-", "Rotate 90 Degrees Left", "");
	geo(s, "sel=5:3:17:11", "Crop", "");
	geo(s, "-", "Canvas Size...", "width=40 height=30 position=Center zero");
	geo(s, "-", "Canvas Size...", "width=36 height=27 position=Bottom-Right");
	geo(s, "-", "Translate...", "x=5 y=-3 interpolation=None");
}
s = "gray8";
geo(s, "sel=3:4:10:7", "Flip Horizontally", "");
geo(s, "sel=3:4:11:8", "Flip Vertically", "");
geo(s, "cal", "Flip Horizontally", "");
geo(s, "cal", "Flip Vertically", "");
geo(s, "cal sel=3:4:10:7", "Flip Horizontally", "");
geo(s, "cal", "Rotate 90 Degrees Right", "");
geo(s, "cal", "Rotate 90 Degrees Left", "");
geo(s, "sel=3:4:10:7", "Rotate 90 Degrees Right", "");
geo(s, "cal sel=5:3:17:11", "Crop", "");
geo(s, "sel=-4:-3:20:15", "Crop", "");
geo(s, "sel=20:15:30:30", "Crop", "");
geo("gray16", "range=1000:50000 sel=5:3:17:11", "Crop", "");
geo(s, "sel=5:3:17:11", "Duplicate...", "title=copy");
geo(s, "sel=5:3:17:11", "Duplicate...", "title=copy ignore");
geo(s, "cal sel=5:3:17:11", "Duplicate...", "title=copy");
geo(s, "-", "Duplicate...", "title=copy");
geo("gray16", "range=1000:50000", "Duplicate...", "title=copy");
geo(s, "-", "Canvas Size...", "width=20 height=15 position=Top-Left zero");
geo(s, "-", "Canvas Size...", "width=20 height=31 position=Center zero");
geo(s, "-", "Canvas Size...", "width=41 height=28 position=Top-Right zero");
geo(s, "-", "Canvas Size...", "width=41 height=28 position=Center-Left zero");
geo(s, "-", "Canvas Size...", "width=41 height=28 position=Bottom-Center zero");
geo(s, "cal", "Canvas Size...", "width=40 height=30 position=Center zero");
geo("gray16", "range=1000:50000", "Canvas Size...", "width=36 height=27 position=Center");
geo("gray32f", "range=-10:20", "Canvas Size...", "width=36 height=27 position=Center");
geo(s, "-", "Translate...", "x=-7 y=4 interpolation=None");
geo(s, "-", "Translate...", "x=2.5 y=1.25 interpolation=None");
geo(s, "-", "Translate...", "x=40 y=0 interpolation=None");
geo(s, "sel=3:4:10:7", "Translate...", "x=2 y=1 interpolation=None");

s = "stack";
geo(s, "slice=2", "Flip Horizontally", "stack");
geo(s, "slice=2", "Flip Vertically", "slice");
geo(s, "slice=2", "Rotate 90 Degrees Right", "");
geo(s, "-", "Flip Z", "");
geo(s, "range=40:180", "Flip Z", "");
geo(s, "sel=5:3:17:11", "Crop", "");
geo(s, "slice=3", "Duplicate...", "title=copy");
geo(s, "slice=3", "Duplicate...", "title=copy duplicate");
geo(s, "slice=3", "Duplicate...", "title=copy duplicate range=2-4");
geo(s, "slice=3 sel=5:3:17:11", "Duplicate...", "title=copy duplicate range=2-4");
geo(s, "slice=3 sel=5:3:17:11", "Duplicate...", "title=copy");
geo(s, "-", "Canvas Size...", "width=36 height=27 position=Center zero");
geo(s, "slice=2", "Translate...", "x=5 y=-3 interpolation=None stack");
geo(s, "slice=2", "Translate...", "x=5 y=-3 interpolation=None slice");

s = "hyper";
geo(s, "-", "Flip Horizontally", "stack");
geo(s, "pos=2:2:1", "Flip Vertically", "slice");
geo(s, "-", "Rotate 90 Degrees Left", "");
geo(s, "sel=5:3:17:11", "Crop", "");
geo(s, "pos=2:2:1", "Duplicate...", "title=copy");
geo(s, "pos=2:2:1", "Duplicate...", "title=copy duplicate");
geo(s, "pos=2:2:1", "Duplicate...", "title=copy duplicate channels=2-3 slices=2 frames=1-2");
geo(s, "pos=2:2:1 sel=5:3:17:11", "Duplicate...", "title=copy duplicate channels=2 frames=2");

for (i=nImages; i>=1; i--) {
	selectImage(i);
	title = getTitle();
	saveAs("Raw Data", d+"/"+title+".raw");
}
File.saveString(csv, d+"/geometry.csv");
print("GEN_OK");

function source(name) {
	if (name=="stack") {
		open(d+"/stack_z6_8bit.tif");
	} else if (name=="hyper") {
		open(d+"/hyperstack_c3z4t5.tif");
		run("Duplicate...", "title=hyper duplicate channels=1-3 slices=1-2 frames=2-3");
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
		if (!startsWith(getTitle(), "geo_")) close();
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
	group = "geo_" + bd + "_" + w + "x" + h;
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
