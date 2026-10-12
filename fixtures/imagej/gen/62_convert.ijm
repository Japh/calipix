// Image > Type conversions.
// Sources:
//   gray8, gray16, gray32f, rgb : the base planes of 01_planes.ijm cropped to the 32x24 rectangle
//                                 at (16,12); gray16 and gray32f then get special values (special())
//   stack  : stack_z6_8bit.tif
//   hyper  : channels 1-3, slices 1-2, frames 2-3 of hyperstack_c3z4t5.tif (12 planes, composite)
//   comp   : channels 1-3 of slice 2, frame 3 of hyperstack_c3z4t5.tif (3 planes, composite)
// Setup words, applied in order before the conversion:
//   range=<min>:<max>  setMinAndMax        lut=<name>   run(<name>)      invertlut   run("Invert LUT")
//   noscale  "Scale when converting" off   weighted     "Weighted RGB conversions" on
//   first=<type>       an earlier conversion (e.g. RGB Stack before RGB Color)
// Outputs:
//   conv_<group>.raw  result planes of each group, in the order of convert.csv
//   convert.csv       group,slice,count,source,setup,command,bits,channels,slices,frames,ranges,
//                     inverting,lut,pixel_width,pixel_height,pixel_depth,unit
//                     ranges: display "min max" of each channel, joined by ";"
//                     lut: red/green/blue of entries 64 and 255 ("-" for RGB images)
var d = getArgument();
var csv = "group,slice,count,source,setup,command,bits,channels,slices,frames,ranges,inverting,lut,pixel_width,pixel_height,pixel_depth,unit\n";
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);

conv("gray8", "-", "16-bit");
conv("gray8", "-", "32-bit");
conv("gray8", "-", "RGB Color");
conv("gray8", "range=50:200", "16-bit");
conv("gray8", "range=50:200", "32-bit");
conv("gray8", "range=50:200", "RGB Color");
conv("gray8", "lut=Fire", "RGB Color");
conv("gray8", "lut=Fire", "16-bit");
conv("gray8", "lut=Fire range=30:180", "RGB Color");
conv("gray8", "invertlut", "RGB Color");
conv("gray8", "invertlut", "32-bit");
conv("gray8", "lut=Fire", "8-bit");

conv("gray16", "-", "8-bit");
conv("gray16", "range=1000:50000", "8-bit");
conv("gray16", "range=20000.4:20255.6", "8-bit");
conv("gray16", "noscale", "8-bit");
conv("gray16", "-", "32-bit");
conv("gray16", "range=1000:50000", "32-bit");
conv("gray16", "-", "RGB Color");
conv("gray16", "range=1000:50000 lut=Fire", "RGB Color");
conv("gray16", "lut=Fire", "8-bit");

conv("gray32f", "-", "8-bit");
conv("gray32f", "range=-10:20", "8-bit");
conv("gray32f", "noscale", "8-bit");
conv("gray32f", "-", "16-bit");
conv("gray32f", "range=-10:20", "16-bit");
conv("gray32f", "range=5:5", "16-bit");
conv("gray32f", "noscale", "16-bit");
conv("gray32f", "range=-10:20", "RGB Color");

conv("rgb", "-", "8-bit");
conv("rgb", "weighted", "8-bit");
conv("rgb", "-", "16-bit");
conv("rgb", "-", "32-bit");
conv("rgb", "weighted", "32-bit");
conv("rgb", "-", "RGB Stack");
conv("rgb", "first=RGB_Stack", "RGB Color");

conv("stack", "-", "16-bit");
conv("stack", "range=40:180", "32-bit");
conv("stack", "range=40:180", "RGB Color");
conv("hyper", "-", "8-bit");
conv("hyper", "noscale", "8-bit");
conv("hyper", "-", "32-bit");
conv("comp", "-", "RGB Color");
conv("comp", "-", "8-bit");

groups = newArray("u8", "u16", "f32", "rgb");
for (i=0; i<groups.length; i++) {
	selectImage("conv_"+groups[i]);
	saveAs("Raw Data", d+"/conv_"+groups[i]+".raw");
	close();
}
File.saveString(csv, d+"/convert.csv");
print("GEN_OK");

// Values the base patterns lack.
function special(base) {
	if (base=="gray32f") {
		setPixel(0, 0, NaN); setPixel(1, 0, 0); setPixel(2, 0, 300.5); setPixel(3, 0, -70000);
		setPixel(4, 0, 1e-30); setPixel(5, 0, 70000.6); setPixel(6, 0, -2.5); setPixel(7, 0, 0.5);
		setPixel(8, 0, 254.5); setPixel(9, 0, 255.49); setPixel(10, 0, 65534.5); setPixel(11, 0, 1.5);
	} else if (base=="gray16") {
		setPixel(0, 0, 0); setPixel(1, 0, 65535); setPixel(2, 0, 255); setPixel(3, 0, 256);
	}
}

function conv(source, setup, command) {
	print("conv "+source+" ["+setup+"] "+command);
	setOption("ScaleConversions", true);
	run("Conversions...", "scale");
	if (source=="stack") {
		open(d+"/stack_z6_8bit.tif");
	} else if (source=="hyper") {
		open(d+"/hyperstack_c3z4t5.tif");
		run("Duplicate...", "title=hyper duplicate channels=1-3 slices=1-2 frames=2-3");
	} else if (source=="comp") {
		open(d+"/hyperstack_c3z4t5.tif");
		run("Duplicate...", "title=comp duplicate channels=1-3 slices=2 frames=3");
	} else {
		open(d+"/"+source+".tif");
		makeRectangle(16, 12, 32, 24);
		run("Crop");
		special(source);
	}
	id = getImageID();
	if (setup!="-") {
		p = split(setup, " ");
		for (i=0; i<p.length; i++) {
			if (startsWith(p[i], "range=")) {
				r = split(substring(p[i], 6), ":");
				setMinAndMax(parseFloat(r[0]), parseFloat(r[1]));
			} else if (startsWith(p[i], "lut="))
				run(substring(p[i], 4));
			else if (p[i]=="invertlut")
				run("Invert LUT");
			else if (p[i]=="noscale")
				setOption("ScaleConversions", false);
			else if (p[i]=="weighted")
				run("Conversions...", "scale weighted");
			else if (startsWith(p[i], "first="))
				run(replace(substring(p[i], 6), "_", " "));
		}
	}
	run(command);
	bd = bitDepth();
	Stack.getDimensions(w, h, c, z, t);
	count = nSlices;
	ranges = "";
	if (c>1 && bd!=24) {
		for (ch=1; ch<=c; ch++) {
			Stack.setChannel(ch);
			getMinAndMax(mn, mx);
			if (ch>1) ranges = ranges + ";";
			ranges = ranges + mn + " " + mx;
		}
		Stack.setChannel(1);
	} else {
		getMinAndMax(mn, mx);
		ranges = "" + mn + " " + mx;
	}
	lut = "-";
	inverting = "-";
	if (bd!=24) {
		getLut(reds, greens, blues);
		lut = "" + reds[64] + "/" + greens[64] + "/" + blues[64] + "/" + reds[255] + "/" + greens[255] + "/" + blues[255];
		inverting = "" + is("Inverting LUT");
	}
	getVoxelSize(pw, ph, pd, unit);
	group = "u8";
	if (bd==16) group = "u16";
	if (bd==32) group = "f32";
	if (bd==24) group = "rgb";
	first = collect("conv_"+group);
	csv = csv + group + "," + first + "," + count + "," + source + "," + setup + "," + command + "," + bd + "," + c + "," + z + "," + t + "," + ranges + "," + inverting + "," + lut + "," + pw + "," + ph + "," + pd + "," + unit + "\n";
	// close everything but the result stacks
	for (i=nImages; i>=1; i--) {
		selectImage(i);
		if (!startsWith(getTitle(), "conv_")) close();
	}
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
