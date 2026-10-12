// Reference results for the filters of Process > Filters, Process > Noise, Process > Shadows
// and Process > Smooth / Sharpen / Find Edges.
// (Depends on 01 and 06: the driver runs gen/*.ijm in lexical order.)
//
// Each base image is the top-left 40x30 of an existing fixture. For every base, one raw file
// holds a stack: slice 1 is the base itself and each further slice is the base after ONE
// command, applied to that slice only. filt_index.txt lists what each slice is:
//   file <tab> slice <tab> command <tab> options <tab> selection ("x y w h" or "-")
// Line breaks inside options are written as |.
//
// Bases:  8 = gray8.tif   16 = gray16.tif   32 = gray32f.tif   rgb = rgb.tif mixed with gray8
//         nan = gray32f with NaN, +Infinity and -Infinity samples
//         struct = blobs.tif blurred, on a ramp, with single bright and dark samples
//         8cal = gray8 calibrated 0.5 x 0.25 um per pixel (for "scaled" units)
// Raw files are big-endian; RGB is interleaved; a stack is its slices one after another.
// (Raw rather than TIFF so that the files are exact for float and do not join the TIFF corpus.)
// Outputs: filt_8.raw filt_16.raw filt_32.raw filt_rgb.raw filt_nan.raw filt_struct.raw
//          filt_8cal.raw filt_stack16_base.raw filt_stack16_gauss.raw filt_stack16_median.raw
//          filt_big8_gauss9.raw filt_big32_gauss9.raw filt_big16_unsharp9.raw filt_index.txt
var W = 40;
var H = 30;
var nCases = 0;
var caseBases = newArray(400);
var caseCmds = newArray(400);
var caseOpts = newArray(400);
var caseRois = newArray(400);
var index = "";

function add(bases, cmd, opts, roi) {
	caseBases[nCases] = " " + bases + " ";
	caseCmds[nCases] = cmd;
	caseOpts[nCases] = opts;
	caseRois[nCases] = roi;
	nCases++;
}

// Reads the top-left W x H of the open image into an array and closes it.
function grab() {
	a = newArray(W*H);
	for (y=0; y<H; y++) for (x=0; x<W; x++) a[y*W+x] = getPixel(x, y);
	close();
	return a;
}

function chooseRoi(roi) {
	if (roi == "-") {
		run("Select None");
	} else {
		r = split(roi, " ");
		makeRectangle(parseInt(r[0]), parseInt(r[1]), parseInt(r[2]), parseInt(r[3]));
	}
}

// Builds the stack for one base, applies every case that names it, and saves it.
function process(d, base, type, pixels) {
	n = 1;
	for (i=0; i<nCases; i++) if (indexOf(caseBases[i], " " + base + " ") >= 0) n++;
	newImage("filt_" + base, type + " black", W, H, n);
	for (s=1; s<=n; s++) {
		setSlice(s);
		for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, pixels[y*W+x]);
	}
	if (base == "8cal") setVoxelSize(0.5, 0.25, 1, "um");
	file = "filt_" + base + ".raw";
	index = index + file + "\t1\tbase\t-\t-\n";
	s = 1;
	for (i=0; i<nCases; i++) {
		if (indexOf(caseBases[i], " " + base + " ") >= 0) {
			s++;
			setSlice(s);
			chooseRoi(caseRois[i]);
			run(caseCmds[i], caseOpts[i]);
			index = index + file + "\t" + s + "\t" + caseCmds[i] + "\t" + replace(caseOpts[i], "\n", "|") +"\t" + caseRois[i] + "\n";
		}
	}
	run("Select None");
	saveAs("Raw Data", d + "/" + file);
	close();
}

d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row"); // big-endian raw data
setOption("BlackBackground", true);

R = "6 5 20 14"; // a selection away from every edge

// ---- Gaussian Blur
add("8 16 32 rgb", "Gaussian Blur...", "sigma=1", "-");
add("8 32", "Gaussian Blur...", "sigma=0.5", "-");
add("16 rgb", "Gaussian Blur...", "sigma=3.7", "-");
add("8 16 32 rgb", "Gaussian Blur...", "sigma=10", "-");       // downscaled by 2
add("16 32", "Gaussian Blur...", "sigma=21", "-");             // downscaled by 5
add("32", "Gaussian Blur...", "sigma=1000", "-");              // kernel longer than the line
add("8 16 32 rgb", "Gaussian Blur...", "sigma=2", R);
add("8 32", "Gaussian Blur...", "sigma=12", R);                // downscaling grid follows the selection
add("16", "Gaussian Blur...", "sigma=2", "1 2 3 24");          // narrow, next to the left edge
add("nan struct", "Gaussian Blur...", "sigma=1", "-");
add("8cal", "Gaussian Blur...", "sigma=1 scaled", "-");
add("8cal", "Gaussian Blur...", "sigma=1", "-");

// ---- Rank filters
add("8 16 32 rgb", "Mean...", "radius=2", "-");
add("8 32", "Mean...", "radius=0.5", "-");
add("16 rgb", "Mean...", "radius=5", "-");
add("8 16 32 rgb", "Mean...", "radius=3", R);
add("nan", "Mean...", "radius=2", "-");
add("8 16 32 rgb", "Median...", "radius=2", "-");
add("16 32", "Median...", "radius=1", "-");
add("8 rgb struct", "Median...", "radius=4", "-");
add("8 32", "Median...", "radius=2", R);
add("nan", "Median...", "radius=2", "-");
add("8 16 32 rgb", "Minimum...", "radius=1", "-");
add("16 32 struct", "Minimum...", "radius=3", "-");
add("16", "Minimum...", "radius=2", "5 0 12 30");
add("nan", "Minimum...", "radius=1", "-");
add("8 16 32 rgb", "Maximum...", "radius=1.5", "-");
add("8 rgb", "Maximum...", "radius=3", "-");
add("nan", "Maximum...", "radius=1", "-");
add("8 16 32 rgb", "Variance...", "radius=1", "-");
add("16 32", "Variance...", "radius=2", "-");
add("32", "Variance...", "radius=2", R);
add("nan", "Variance...", "radius=1", "-");
add("8 16 32 rgb struct", "Despeckle", "slice", "-");
add("8 rgb struct", "Remove Outliers...", "radius=2 threshold=50 which=Bright", "-");
add("16", "Remove Outliers...", "radius=1 threshold=200 which=Bright", "-");
add("32", "Remove Outliers...", "radius=2 threshold=2 which=Bright", "-");
add("8 struct", "Remove Outliers...", "radius=1 threshold=20 which=Dark", "-");
add("32", "Remove Outliers...", "radius=1 threshold=1.5 which=Dark", "-");
add("16", "Remove Outliers...", "radius=2 threshold=300 which=Dark", R);
add("nan", "Remove Outliers...", "radius=2 threshold=1 which=Bright", "-");
add("nan", "Remove NaNs...", "radius=2", "-");
add("nan", "Remove NaNs...", "radius=0.5", "-");
add("nan", "Remove NaNs...", "radius=1", R);
add("8 16 32 rgb struct", "Top Hat...", "radius=3", "-");
add("8 16 32 rgb", "Top Hat...", "radius=3 light", "-");
add("8 16", "Top Hat...", "radius=2 don't", "-");
add("32", "Top Hat...", "radius=2 light don't", "-");
add("8 16", "Top Hat...", "radius=3", R);
add("32", "Top Hat...", "radius=2 light", "0 0 40 12");
add("nan", "Top Hat...", "radius=2", "-");

// ---- Convolve
add("8 16 32 rgb", "Convolve...", "text1=[-1 -1 -1 -1 -1\n-1 -1 -1 -1 -1\n-1 -1 24 -1 -1\n-1 -1 -1 -1 -1\n-1 -1 -1 -1 -1\n] normalize", "-");
add("8 16 32 rgb struct", "Convolve...", "text1=[1 2 1\n2 4 2\n1 2 1\n] normalize", "-");
add("8 16 32", "Convolve...", "text1=[0 -1 0\n-1 5 -1\n0 -1 0\n]", "-");
add("16", "Convolve...", "text1=[1 2 1\n2 4 2\n1 2 1\n]", "-"); // not normalized: clamps
add("8 32", "Convolve...", "text1=[1 2 3 2 1\n] normalize", "-");
add("16 32", "Convolve...", "text1=[1\n2\n3\n4\n3\n2\n1\n] normalize", "-");
add("16", "Convolve...", "text1=[1 0 -1 2 0\n0 1 0.5 3 1\n2 0 0 1 0\n] normalize", "-");
add("8", "Convolve...", "text1=[1 1 1 1 1 1 1 1 1] normalize", "-"); // one line: read as 3x3
add("8 32", "Convolve...", "text1=[1 2 1\n2 4 2\n1 2 1\n] normalize", R);
add("nan", "Convolve...", "text1=[1 2 1\n2 4 2\n1 2 1\n] normalize", "-");

// ---- Unsharp Mask
add("8 16 32 rgb", "Unsharp Mask...", "radius=1 mask=0.60", "-");
add("8 16 32 rgb struct", "Unsharp Mask...", "radius=3 mask=0.30", "-");
add("8 32", "Unsharp Mask...", "radius=10 mask=0.50", "-");
add("16 rgb", "Unsharp Mask...", "radius=2 mask=0.60", R);

// ---- 3x3 filters
add("8 16 32 rgb struct nan", "Smooth", "slice", "-");
add("8 16 32 rgb struct nan", "Sharpen", "slice", "-");
add("8 16 32 rgb struct nan", "Find Edges", "slice", "-");
add("8 rgb", "Smooth", "slice", R);
add("16 rgb", "Sharpen", "slice", R);
add("rgb", "Sharpen", "slice", "0 0 14 30");
add("32 rgb", "Find Edges", "slice", "0 4 40 12");
add("8 16 32 rgb", "North", "slice", "-");
add("8 16 32 rgb", "Southeast", "slice", "-");
add("16 rgb", "Northeast", "slice", "-");
add("16 rgb", "East", "slice", "-");
add("8", "South", "slice", "-");
add("8", "Southwest", "slice", "-");
add("8", "West", "slice", "-");
add("8", "Northwest", "slice", "-");
add("rgb", "West", "slice", R);

// ---- Bases
open(d + "/gray8.tif");
gray8 = grab();
open(d + "/gray16.tif");
gray16 = grab();
open(d + "/gray32f.tif");
gray32 = grab();
open(d + "/rgb.tif");
rgb = grab();
// Within 40x30 the channels of rgb.tif are plain ramps, which most filters leave as they are,
// so each channel is mixed with the gray8 pattern.
for (i=0; i<W*H; i++) {
	r = (((rgb[i] >> 16) & 255) + gray8[i]) & 255;
	g = (((rgb[i] >> 8) & 255) * 3 + (gray8[i] >> 1)) & 255;
	b = (rgb[i] & 255) ^ (gray8[i] & 60);
	rgb[i] = (r << 16) | (g << 8) | b;
}

nan = Array.copy(gray32);
for (y=12; y<17; y++) for (x=12; x<17; x++) nan[y*W+x] = NaN;
nan[0] = NaN; nan[H*W-1] = NaN;
nan[5*W+5] = NaN; nan[5*W+6] = NaN;
nan[10*W+25] = NaN; nan[11*W+25] = NaN; nan[10*W+26] = NaN; nan[11*W+26] = NaN;
nan[20*W+30] = 1/0;
nan[22*W+10] = -1/0;
nan[3*W+33] = 1e30;

// A structured 8-bit image: soft-edged blobs on a ramp, with isolated bright and dark samples.
open(d + "/blobs.tif");
makeRectangle(0, 0, W, H);
run("Crop");
run("Gaussian Blur...", "sigma=1.5");
for (y=0; y<H; y++) for (x=0; x<W; x++) {
	v = getPixel(x, y) * 0.7 + x + y / 2;
	if ((x*7 + y*13) % 97 == 0) v = 250;
	if ((x*11 + y*5) % 89 == 3) v = 4;
	setPixel(x, y, floor(v));
}
struct = grab();

process(d, "8", "8-bit", gray8);
process(d, "16", "16-bit", gray16);
process(d, "32", "32-bit", gray32);
process(d, "rgb", "RGB", rgb);
process(d, "nan", "32-bit", nan);
process(d, "struct", "8-bit", struct);
process(d, "8cal", "8-bit", gray8);

// ---- The "stack" keyword: every slice is filtered
function makeStack() {
	newImage("stack16", "16-bit black", W, H, 3);
	for (s=1; s<=3; s++) {
		setSlice(s);
		for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, (x*257 + y*y*31 + 7001*s) % 65536);
	}
	setSlice(2);
}
makeStack();
saveAs("Raw Data", d + "/filt_stack16_base.raw");
close();
makeStack();
run("Gaussian Blur...", "sigma=1.5 stack");
saveAs("Raw Data", d + "/filt_stack16_gauss.raw");
close();
makeStack();
makeRectangle(6, 5, 20, 14);
run("Median...", "radius=2 stack");
saveAs("Raw Data", d + "/filt_stack16_median.raw");
close();

// ---- A large Gaussian in a selection of the full 64x48 images. Above sigma 8.5 ImageJ blurs
// a downscaled copy whose grid starts a kernel radius before the selection, so these differ
// from the same samples of the blurred whole image. (The 40x30 bases are too small to show it.)
open(d + "/gray8.tif");
makeRectangle(41, 37, 20, 10);
run("Gaussian Blur...", "sigma=9");
run("Select None");
saveAs("Raw Data", d + "/filt_big8_gauss9.raw");
close();
open(d + "/gray32f.tif");
makeRectangle(41, 41, 20, 6);
run("Gaussian Blur...", "sigma=9");
run("Select None");
saveAs("Raw Data", d + "/filt_big32_gauss9.raw");
close();
open(d + "/gray16.tif");
makeRectangle(43, 40, 20, 8);
run("Unsharp Mask...", "radius=9 mask=0.60");
run("Select None");
saveAs("Raw Data", d + "/filt_big16_unsharp9.raw");
close();

File.saveString(index, d + "/filt_index.txt");
print("GEN_OK");
