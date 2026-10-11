// PNG reading and writing.
//
// Reading: what ImageJ makes of the hand-built PNG files in gen/22_png_inputs, which hold every
// colour type and bit depth, palettes, transparency and interlacing. The inputs are built by
// crates/calipix-io/tests/png_craft.rs.
// Outputs, for each input <name>.png:
//   png_<name>_<s>.txt     8-bit and 16-bit results: sample values of slice s as a text image
//                          RGB results: the red (s=1), green (2) and blue (3) values
//   png_<name>_lut<s>.csv  the lookup table of slice s of an 8-bit or 16-bit result, one "r,g,b"
//                          line per entry; not written when it is the 256-level grey ramp
//   png_info.csv           name,bitDepth,width,height,channels,slices,frames,composite,invertingLut
//
// Writing: PNG files saved by ImageJ, as pngw_<name>.png. The image each was saved from is
// described in the same way, by pngw_<name>_<s>.txt, pngw_<name>_lut<s>.csv and pngw_info.csv.
//   gray8     8-bit                          gray16    16-bit
//   fire8     8-bit with the Fire LUT        range16   16-bit with a display range of 1000-20000
//   inv8      8-bit with an inverting LUT    fire16    16-bit with the Fire LUT
//   range8    8-bit with a display range     rgb       RGB
//             of 50-150
//   stack8    8-bit stack of 3 slices, showing slice 2
//   comp16    16-bit composite of 3 channels
//   float     32-bit, values x/2-y/4 (not described)
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);

// The inputs sit beside this macro; the output directory is either fixtures/imagej/out (gen)
// or target/verify-out (verify).
src = File.getParent(d)+"/gen/22_png_inputs/";
if (!File.exists(src+"gray8.png"))
	src = File.getParent(File.getParent(d))+"/fixtures/imagej/gen/22_png_inputs/";
if (!File.exists(src+"gray8.png")) {
	print("cannot find 22_png_inputs from "+d);
	exit;
}

names = newArray("gray1", "gray2", "gray4", "gray8", "gray16", "graya8", "graya16", "rgb8", "rgb16",
	"rgba8", "rgba16", "pal1", "pal2", "pal4", "pal8", "pal8_gray", "pal4_gray", "pal1_gray",
	"pal8_invgray", "pal8_short", "pal4_short", "pal8_grayshort", "rgb8_gray", "pal8_trns",
	"pal4_trns", "pal1_trns", "pal8_trns_opaque", "gray8_trns", "gray16_trns", "gray4_trns_zero",
	"gray4_trns", "rgb8_trns", "rgb16_trns", "rgba8_gama_bkgd", "gray1_i", "gray8_i", "gray16_i",
	"rgb8_i", "rgba8_i", "pal4_i", "blend_rgba8", "blend_rgba16", "blend_graya8", "blend_graya16");
info = "";
for (i=0; i<names.length; i++) {
	open(src+names[i]+".png");
	info = info+names[i]+","+describe(d+"/png_"+names[i])+"\n";
	close();
}
File.saveString(info, d+"/png_info.csv");

// Writing.
info = "";
newImage("a", "8-bit black", 16, 9, 1);
fill(0, 256);
info = info+both(d, "gray8");
run("Fire");
info = info+both(d, "fire8");
close();
newImage("a", "8-bit black", 16, 9, 1);
fill(0, 256);
run("Invert LUT");
info = info+both(d, "inv8");
close();
newImage("a", "8-bit black", 16, 9, 1);
fill(0, 256);
setMinAndMax(50, 150);
info = info+both(d, "range8");
close();
newImage("a", "16-bit black", 16, 9, 1);
fill(0, 65536);
info = info+both(d, "gray16");
setMinAndMax(1000, 20000);
info = info+both(d, "range16");
run("Fire");
info = info+both(d, "fire16");
close();
newImage("a", "RGB black", 16, 9, 1);
for (y=0; y<9; y++)
	for (x=0; x<16; x++) {
		setColor((x*37)%256, (y*91)%256, (x*y*5)%256);
		drawLine(x, y, x, y);
	}
info = info+both(d, "rgb");
close();
newImage("a", "8-bit black", 16, 9, 3);
for (s=1; s<=3; s++) {
	setSlice(s);
	fill(s*50, 256);
}
info = info+"stack8,"+describe(d+"/pngw_stack8")+"\n";
setSlice(2);
saveAs("PNG", d+"/pngw_stack8.png");
close();
newImage("a", "16-bit composite-mode", 16, 9, 3, 1, 1);
for (s=1; s<=3; s++) {
	Stack.setChannel(s);
	fill(s*5000, 65536);
}
info = info+both(d, "comp16");
close();
newImage("a", "32-bit black", 16, 9, 1);
for (y=0; y<9; y++)
	for (x=0; x<16; x++)
		setPixel(x, y, x*0.5-y*0.25);
saveAs("PNG", d+"/pngw_float.png");
close();
File.saveString(info, d+"/pngw_info.csv");
print("GEN_OK");

// Writes the sample values and lookup tables of the current image to files starting with
// `base`, and returns the rest of its line of the info file.
function describe(base) {
	getDimensions(w, h, c, z, t);
	depth = bitDepth();
	line = ""+depth+","+w+","+h+","+c+","+z+","+t+","+is("composite")+","+is("Inverting LUT");
	id = getImageID();
	if (depth==24) {
		run("Duplicate...", "title=copy");
		run("RGB Stack");
		copy = getImageID();
		for (s=1; s<=3; s++) {
			selectImage(copy);
			setSlice(s);
			run("Duplicate...", "title=slice");
			saveAs("Text Image", base+"_"+s+".txt");
			close();
		}
		selectImage(copy);
		close();
	} else {
		n = nSlices;
		for (s=1; s<=n; s++) {
			selectImage(id);
			setSlice(s);
			getLut(r, g, b);
			ramp = true;
			if (r.length!=256) ramp = false;
			lut = "";
			for (k=0; k<r.length; k++) {
				if (r[k]!=k || g[k]!=k || b[k]!=k) ramp = false;
				lut = lut+r[k]+","+g[k]+","+b[k]+"\n";
			}
			if (ramp==false) File.saveString(lut, base+"_lut"+s+".csv");
			run("Duplicate...", "title=slice");
			saveAs("Text Image", base+"_"+s+".txt");
			close();
		}
	}
	selectImage(id);
	return line;
}

// Describes the current image and saves it as PNG; returns its line of the info file.
function both(d, name) {
	line = name+","+describe(d+"/pngw_"+name)+"\n";
	saveAs("PNG", d+"/pngw_"+name+".png");
	return line;
}

// Fills the current slice with values that depend on the position.
function fill(offset, modulus) {
	for (y=0; y<getHeight(); y++)
		for (x=0; x<getWidth(); x++)
			setPixel(x, y, (x*5003+y*7919+x*y+offset)%modulus);
	updateDisplay();
}
