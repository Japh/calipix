// Process > Image Calculator.
// Image 1 ("A") is a base plane from 01_planes.ijm cropped to the 32x24 rectangle at (16,12),
// or stack_z6_8bit.tif (which is 32x24). Image 2 is one of these generated 32x24 images, which the
// Rust tests rebuild from the same formulas (display range reset to the data range):
//   b8   : v = (x*7 + y*13) mod 256,       0 where x mod 5 == 0 and y mod 3 == 0
//   b16  : v = (x*2021 + y*517) mod 65536, 0 where x mod 5 == 0 and y mod 3 == 0, 65535 at (1,0)
//   bf   : v = (float)((x-10)*0.5 - (y mod 7) + 0.25*(x mod 3)), NaN at (2,1)
//   brgb : r = (x*9)&255, g = (y*11)&255, b = (x*y)&255, black where x mod 6 == 0 and y mod 4 == 0
//   small8 (20x10): v = (x*11 + y*3) mod 256
//   bstack: stack_z6_8bit.tif after Multiply 0.5 on every slice
// Outputs:
//   calc_<group>.raw  the result planes of each group, in the order of calc.csv
//   calc.csv          group,slice,count,image1,image2,options,width,height,bits,min,max
//                     slice = first plane of the case within the group, count = planes of the case;
//                     options are imageCalculator's, plus "divide=<v>" for the "Divide by zero
//                     value" preference and "slice=<n>" for the slice of image 1 that was current
var d = getArgument();
var csv = "group,slice,count,image1,image2,options,width,height,bits,min,max\n";
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
setOption("ScaleConversions", true);
W = 32; H = 24;

newImage("b8", "8-bit black", W, H, 1);
for (y=0; y<H; y++) for (x=0; x<W; x++) {
	v = (x*7 + y*13) % 256;
	if (x%5==0 && y%3==0) v = 0;
	setPixel(x, y, v);
}
newImage("b16", "16-bit black", W, H, 1);
for (y=0; y<H; y++) for (x=0; x<W; x++) {
	v = (x*2021 + y*517) % 65536;
	if (x%5==0 && y%3==0) v = 0;
	setPixel(x, y, v);
}
setPixel(1, 0, 65535);
resetMinAndMax();
newImage("bf", "32-bit black", W, H, 1);
for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, (x-10)*0.5 - (y%7) + 0.25*(x%3));
setPixel(2, 1, NaN);
resetMinAndMax();
newImage("brgb", "RGB black", W, H, 1);
for (y=0; y<H; y++) for (x=0; x<W; x++) {
	r = (x*9)&255; g = (y*11)&255; b = (x*y)&255;
	if (x%6==0 && y%4==0) { r = 0; g = 0; b = 0; }
	setPixel(x, y, (r<<16) | (g<<8) | b);
}
newImage("small8", "8-bit black", 20, 10, 1);
for (y=0; y<10; y++) for (x=0; x<20; x++) setPixel(x, y, (x*11 + y*3) % 256);
open(d+"/stack_z6_8bit.tif");
rename("bstack");
run("Multiply...", "value=0.5 stack");

ops = newArray("Add", "Subtract", "Multiply", "Divide", "AND", "OR", "XOR", "Min", "Max", "Average", "Difference", "Copy", "Transparent-zero");
bases = newArray("gray8", "gray16", "gray32f", "rgb");
seconds = newArray("b8", "b16", "bf", "brgb");
for (i=0; i<bases.length; i++) {
	for (j=0; j<ops.length; j++) {
		// (an array element followed by "+" is not accepted as a function argument)
		options = ops[j] + " create";
		calc(bases[i], bases[i], seconds[i], options);
	}
}

// the "Divide by zero value" preference only affects float division
run("Misc...", "divide=NaN");
calc("gray32f", "gray32f", "bf", "Divide create divide=NaN");
run("Misc...", "divide=0");
calc("gray32f", "gray32f", "bf", "Divide create divide=0");
run("Misc...", "divide=-7.5");
calc("gray32f", "gray32f", "bf", "Divide create divide=-7.5");
calc("gray32f", "gray32f", "bf", "Divide divide=-7.5");
run("Misc...", "divide=Infinity");

// image 2 of another type is converted to the type of image 1
m = "mixed_u8";
calc(m, "gray8", "b16", "Add create");
calc(m, "gray8", "b16", "Multiply create");
calc(m, "gray8", "b16", "Copy create");
calc(m, "gray8", "bf", "Add create");
calc(m, "gray8", "bf", "Multiply create");
calc(m, "gray8", "bf", "Divide create");
calc(m, "gray8", "bf", "Min create");
calc(m, "gray8", "brgb", "Add create");
calc(m, "gray8", "brgb", "Copy create");
m = "mixed_u16";
calc(m, "gray16", "b8", "Add create");
calc(m, "gray16", "b8", "Subtract create");
calc(m, "gray16", "bf", "Add create");
calc(m, "gray16", "bf", "Multiply create");
calc(m, "gray16", "bf", "Divide create");
calc(m, "gray16", "bf", "Copy create");
calc(m, "gray16", "brgb", "Add create");
calc(m, "gray16", "b16", "Subtract");
m = "mixed_f32";
calc(m, "gray32f", "b8", "Add create");
calc(m, "gray32f", "b8", "Divide create");
calc(m, "gray32f", "b16", "Multiply create");
calc(m, "gray32f", "brgb", "Add create");
// "32-bit (float) result"
calc(m, "gray8", "b8", "Add create 32-bit");
calc(m, "gray8", "b8", "Divide create 32-bit");
calc(m, "gray8", "b8", "Multiply create 32-bit");
calc(m, "gray16", "b8", "Subtract create 32-bit");
calc(m, "gray8", "bf", "Subtract create 32-bit");
calc(m, "gray8", "b8", "Divide 32-bit");
calc(m, "gray32f", "bf", "Add 32-bit");
calc(m, "rgb", "brgb", "Add create 32-bit");
calc(m, "gray8", "brgb", "Add create 32-bit");
m = "mixed_rgb";
calc(m, "rgb", "b8", "Add create");
calc(m, "rgb", "b8", "Copy create");
calc(m, "rgb", "b16", "Add create");
calc(m, "rgb", "bf", "Subtract create");
// with an RGB image 1 and a grey image 2 the result stays RGB
calc(m, "rgb", "b8", "Add create 32-bit");

// images of different sizes
calc("small_u8", "gray8", "small8", "Add create");
calc("small_u8", "small8", "b8", "Subtract create");
calc("mixed_u8", "gray8", "small8", "Add");
calc("mixed_u8", "gray8", "small8", "Copy");

// stacks
s = "stack_u8";
calc(s, "stack_z6_8bit", "b8", "Add create stack");
calc(s, "stack_z6_8bit", "bstack", "Difference create stack");
calc(s, "stack_z6_8bit", "b8", "Subtract stack");
calc(s, "stack_z6_8bit", "b8", "Add create slice=3");
calc(s, "stack_z6_8bit", "b8", "Add slice=3");
calc(s, "stack_z6_8bit", "bstack", "Add create slice=3");
calc(s, "b8", "bstack", "Add create stack");
calc("stack_f32", "stack_z6_8bit", "bstack", "Divide create 32-bit stack");
calc("stack_f32", "stack_z6_8bit", "bf", "Multiply create 32-bit stack");

groups = newArray("gray8", "gray16", "gray32f", "rgb", "mixed_u8", "mixed_u16", "mixed_f32", "mixed_rgb", "small_u8", "stack_u8", "stack_f32");
for (i=0; i<groups.length; i++) {
	selectImage("calc_"+groups[i]);
	saveAs("Raw Data", d+"/calc_"+groups[i]+".raw");
	close();
}
File.saveString(csv, d+"/calc.csv");
print("GEN_OK");

// Values the base patterns lack.
function special(base) {
	if (base=="gray32f") {
		setPixel(0, 0, NaN); setPixel(1, 0, 0); setPixel(2, 0, 1/0); setPixel(3, 0, -1/0);
		setPixel(4, 0, 1e-30); setPixel(5, 0, 3.4e38); setPixel(6, 0, -2.5); setPixel(7, 0, 0.5);
	} else if (base=="gray16") {
		setPixel(0, 0, 0); setPixel(1, 0, 65535); setPixel(2, 0, 1); setPixel(3, 0, 32768);
	}
}

function calc(group, name1, name2, options) {
	print("calc "+group+": "+name1+" "+name2+" "+options);
	if (name1=="b8" || name1=="small8") {
		selectImage(name1);
		run("Duplicate...", "title=A");
	} else {
		open(d+"/"+name1+".tif");
		if (nSlices==1) {
			makeRectangle(16, 12, 32, 24);
			run("Crop");
			special(name1);
		}
		rename("A");
	}
	// imageCalculator ignores the keywords it does not know
	p = split(options, " ");
	for (i=0; i<p.length; i++)
		if (startsWith(p[i], "slice=")) setSlice(parseInt(substring(p[i], 6)));
	imageCalculator(options, "A", name2);
	created = indexOf(options, "create")>=0 || (indexOf(options, "32-bit")>=0 && !(bitDepth()==32 && name2=="bf"));
	if (created)
		selectImage("Result of A");
	else
		selectImage("A");
	getMinAndMax(mn, mx);
	count = nSlices;
	w = getWidth(); h = getHeight(); bd = bitDepth();
	first = collect("calc_"+group);
	csv = csv + group + "," + first + "," + count + "," + name1 + "," + name2 + "," + options + "," + w + "," + h + "," + bd + "," + mn + "," + mx + "\n";
	if (created) {
		selectImage("Result of A");
		close();
	}
	selectImage("A");
	close();
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
