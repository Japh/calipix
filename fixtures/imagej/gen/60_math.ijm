// Process > Math applied to the base planes written by 01_planes.ijm.
// Each case opens a fresh copy of a base image, crops it to the 32x24 rectangle at (16,12) to
// keep the references small, gives it some special values (see special()), optionally sets a
// display range, runs one command and appends the resulting plane to a stack named after the
// base image.
// Outputs:
//   math_<base>.raw   one plane per case, in the order of math.csv (raw, big-endian; RGB as r,g,b bytes)
//   math.csv          base,slice,command,options,range_before,min_after,max_after
//                     range_before is "-" (as opened) or "min max" set with setMinAndMax before the command
var d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
setOption("ScaleConversions", true);
var csv = "base,slice,command,options,range_before,min_after,max_after\n";

b = "gray8";
u(b, "Add...", "value=25", "-");
u(b, "Add...", "value=300", "-");
u(b, "Add...", "value=-50", "-");
u(b, "Add...", "value=2.7", "-");
u(b, "Subtract...", "value=25", "-");
u(b, "Subtract...", "value=2.7", "-");
u(b, "Multiply...", "value=1.25", "-");
u(b, "Multiply...", "value=3", "-");
u(b, "Multiply...", "value=-1", "-");
u(b, "Multiply...", "value=0.5", "-");
u(b, "Divide...", "value=3", "-");
u(b, "Divide...", "value=0", "-");
u(b, "Divide...", "value=0.3", "-");
u(b, "AND...", "value=11110000", "-");
u(b, "OR...", "value=1010101", "-");
u(b, "XOR...", "value=11111111", "-");
u(b, "Min...", "value=100", "-");
u(b, "Min...", "value=100.7", "-");
u(b, "Max...", "value=100", "-");
u(b, "Max...", "value=100.7", "-");
u(b, "Gamma...", "value=0.5", "-");
u(b, "Gamma...", "value=2.2", "-");
u(b, "Set...", "value=77", "-");
u(b, "Set...", "value=300", "-");
u(b, "Set...", "value=-5", "-");
u(b, "Set...", "value=2.9", "-");
u(b, "Log", "", "-");
u(b, "Exp", "", "-");
u(b, "Square", "", "-");
u(b, "Square Root", "", "-");
u(b, "Abs", "", "-");
u(b, "Reciprocal", "", "-");
u(b, "Gamma...", "value=0.5", "50 200");
u(b, "Log", "", "50 200");
finish(b);

b = "gray16";
u(b, "Add...", "value=25", "-");
u(b, "Add...", "value=70000", "-");
u(b, "Add...", "value=-500", "-");
u(b, "Add...", "value=2.7", "-");
u(b, "Add...", "value=3000000000", "-");
u(b, "Subtract...", "value=2500", "-");
u(b, "Subtract...", "value=2.7", "-");
u(b, "Multiply...", "value=1.25", "-");
u(b, "Multiply...", "value=3", "-");
u(b, "Multiply...", "value=-1", "-");
u(b, "Multiply...", "value=0.5", "-");
u(b, "Divide...", "value=3", "-");
u(b, "Divide...", "value=0", "-");
u(b, "Divide...", "value=0.3", "-");
u(b, "AND...", "value=1111111100000000", "-");
u(b, "OR...", "value=1010101", "-");
u(b, "XOR...", "value=1111111111111111", "-");
u(b, "Min...", "value=20000", "-");
u(b, "Min...", "value=20000.7", "-");
u(b, "Max...", "value=20000", "-");
u(b, "Max...", "value=20000.7", "-");
u(b, "Gamma...", "value=0.5", "-");
u(b, "Gamma...", "value=2.2", "-");
u(b, "Set...", "value=777", "-");
u(b, "Set...", "value=70000", "-");
u(b, "Set...", "value=-5", "-");
u(b, "Set...", "value=2.9", "-");
u(b, "Log", "", "-");
u(b, "Exp", "", "-");
u(b, "Square", "", "-");
u(b, "Square Root", "", "-");
u(b, "Abs", "", "-");
u(b, "Reciprocal", "", "-");
u(b, "Gamma...", "value=0.5", "1000 50000");
u(b, "Gamma...", "value=2.2", "1000 50000");
u(b, "Log", "", "1000 50000");
u(b, "Exp", "", "1000 50000");
u(b, "Exp", "", "0 1000");
finish(b);

b = "gray32f";
u(b, "Add...", "value=25", "-");
u(b, "Add...", "value=-0.3", "-");
u(b, "Subtract...", "value=2.7", "-");
u(b, "Multiply...", "value=1.25", "-");
u(b, "Multiply...", "value=-3", "-");
u(b, "Divide...", "value=3", "-");
u(b, "Divide...", "value=0", "-");
u(b, "Divide...", "value=0.3", "-");
u(b, "AND...", "value=11110000", "-");
u(b, "OR...", "value=1010101", "-");
u(b, "XOR...", "value=11111111", "-");
u(b, "Min...", "value=0.5", "-");
u(b, "Min...", "value=-3.2", "-");
u(b, "Max...", "value=0.5", "-");
u(b, "Max...", "value=-3.2", "-");
u(b, "Gamma...", "value=0.5", "-");
u(b, "Gamma...", "value=2.2", "-");
u(b, "Set...", "value=77", "-");
u(b, "Set...", "value=-5.3", "-");
u(b, "Log", "", "-");
u(b, "Exp", "", "-");
u(b, "Square", "", "-");
u(b, "Square Root", "", "-");
u(b, "Abs", "", "-");
u(b, "Reciprocal", "", "-");
u(b, "NaN Background", "", "threshold -5 10");
u(b, "NaN Background", "", "threshold 0 1e30");
finish(b);

b = "rgb";
u(b, "Add...", "value=25", "-");
u(b, "Add...", "value=-50", "-");
u(b, "Subtract...", "value=25", "-");
u(b, "Multiply...", "value=1.25", "-");
u(b, "Divide...", "value=3", "-");
u(b, "Divide...", "value=0", "-");
u(b, "AND...", "value=11110000", "-");
u(b, "OR...", "value=1010101", "-");
u(b, "XOR...", "value=11111111", "-");
u(b, "Min...", "value=100", "-");
u(b, "Max...", "value=100", "-");
u(b, "Gamma...", "value=0.5", "-");
u(b, "Set...", "value=77", "-");
u(b, "Set...", "value=1193046", "-");
u(b, "Log", "", "-");
u(b, "Exp", "", "-");
u(b, "Square", "", "-");
u(b, "Square Root", "", "-");
u(b, "Abs", "", "-");
finish(b);

File.saveString(csv, d+"/math.csv");
print("GEN_OK");

// Values the base patterns lack: NaN, zeros, infinities and extremes.
function special(base) {
	if (base=="gray32f") {
		setPixel(0, 0, NaN); setPixel(1, 0, 0); setPixel(2, 0, 1/0); setPixel(3, 0, -1/0);
		setPixel(4, 0, 1e-30); setPixel(5, 0, 3.4e38); setPixel(6, 0, -2.5); setPixel(7, 0, 0.5);
	} else if (base=="gray16") {
		setPixel(0, 0, 0); setPixel(1, 0, 65535); setPixel(2, 0, 1); setPixel(3, 0, 32768);
	}
}

function u(base, command, options, range) {
	open(d+"/"+base+".tif");
	makeRectangle(16, 12, 32, 24);
	run("Crop");
	special(base);
	if (startsWith(range, "threshold ")) {
		p = split(range, " ");
		setThreshold(parseFloat(p[1]), parseFloat(p[2]));
	} else if (range!="-") {
		p = split(range, " ");
		setMinAndMax(parseFloat(p[0]), parseFloat(p[1]));
	}
	run(command, options);
	getMinAndMax(mn, mx);
	n = collect("math_"+base);
	csv = csv + base + "," + n + "," + command + "," + options + "," + range + "," + mn + "," + mx + "\n";
	close();
}

// Appends the current plane of the current image to the stack called 'group' and returns
// its slice number there.
function collect(group) {
	id = getImageID();
	w = getWidth(); h = getHeight(); bd = bitDepth();
	a = newArray(w*h);
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
	n = nSlices;
	selectImage(id);
	return n;
}

function finish(base) {
	selectImage("math_"+base);
	saveAs("Raw Data", d+"/math_"+base+".raw");
	close();
}
