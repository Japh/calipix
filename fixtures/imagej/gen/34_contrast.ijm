// Display ranges chosen by Process > Enhance Contrast (ContrastEnhancer.stretchHistogram).
// The B&C "Auto" button (ContrastAdjuster.autoAdjust) needs the B&C window and cannot be
// driven from a batch macro; it is covered by source-derived tests only.
// Outputs:
//   contrast.csv   image,scope,saturated,min,max
//     scope "plane": run("Enhance Contrast", "saturated=<s>") on a single image
//     scope "stack": the same with "use" (use stack histogram) on slice 1 of a 3-slice stack
//     min and max are getMinAndMax() afterwards; for a new image they start at the default
//     range, so an unchanged range means ImageJ left the display range alone.
d = getArgument();
s = "";
kinds = newArray("u8", "u16", "u16skew", "f32", "f32s", "rgb");
sats = newArray("0.35", "0", "5", "30", "100");
for (k=0; k<kinds.length; k++) {
	for (j=0; j<sats.length; j++) {
		make(kinds[k], 1);
		run("Enhance Contrast", "saturated="+sats[j]);
		getMinAndMax(min, max);
		s = s + kinds[k] + ",plane," + sats[j] + "," + d2s(min, -12) + "," + d2s(max, -12) + "\n";
		close();
	}
}
for (k=0; k<kinds.length; k++) {
	if (kinds[k]!="rgb") {
		make(kinds[k], 3);
		run("Enhance Contrast", "saturated=0.35 use");
		getMinAndMax(min, max);
		s = s + kinds[k] + ",stack,0.35," + d2s(min, -12) + "," + d2s(max, -12) + "\n";
		close();
	}
}
File.saveString(s, d+"/contrast.csv");
print("GEN_OK");

// Test images, 64x48 (z = 0 for single planes; stacks have z = 0..2 and use X = x+7*z, Y = y+3*z):
//   u8      v = (X*X + 3*Y*Y + X*Y) mod 256
//   u16     v = (X*257 + Y*Y*31 + 1000*((floor(X/8)+floor(Y/8)) mod 2)) mod 65536
//   u16skew v = 100 + floor(((X*Y) mod 97)^2 / 3)
//   f32     v = (float)((X-31.5)*0.25 + (Y*Y)/64 - 3.5*((X+Y) mod 5))
//   f32s    as f32, with NaN at (3,0) and (5,7), +Infinity at (20,1) and -Infinity at (10,2)
//   rgb     r = (X*4)&255, g = (Y*5)&255, b = ((X+Y)*3)&255
function make(kind, slices) {
	type = "8-bit";
	if (startsWith(kind, "u16")) type = "16-bit";
	if (startsWith(kind, "f32")) type = "32-bit";
	if (kind=="rgb") type = "RGB";
	newImage(kind, type+" black", 64, 48, slices);
	for (z=0; z<slices; z++) {
		setSlice(z+1);
		for (y=0; y<48; y++) for (x=0; x<64; x++) {
			X = x+7*z; Y = y+3*z;
			if (kind=="u8") v = (X*X + 3*Y*Y + X*Y)%256;
			else if (kind=="u16") v = (X*257 + Y*Y*31 + 1000*((floor(X/8)+floor(Y/8))%2))%65536;
			else if (kind=="u16skew") v = 100 + floor(pow((X*Y)%97, 2)/3);
			else if (kind=="rgb") v = (((X*4)&255)<<16) | (((Y*5)&255)<<8) | (((X+Y)*3)&255);
			else v = (X-31.5)*0.25 + (Y*Y)/64.0 - 3.5*((X+Y)%5);
			setPixel(x, y, v);
		}
		if (kind=="f32s") {
			setPixel(3, 0, NaN); setPixel(5, 7, NaN); setPixel(20, 1, 1/0); setPixel(10, 2, -1/0);
		}
	}
	setSlice(1);
	resetMinAndMax();
}
