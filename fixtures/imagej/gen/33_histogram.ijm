// Histograms and statistics as ImageJ computes them (ByteStatistics, ShortStatistics,
// FloatStatistics, ColorStatistics and StackStatistics).
// Outputs:
//   histograms.csv   one record per line: image,key,values...
//     stats            pixel count, mean, min, max, standard deviation, mode (getValue("Mode"))
//     counts<N>        getHistogram(values, counts, N)
//     values<N>        the bin start values returned with it
//     counts<N>_<a>_<b> / values<N>_<a>_<b>   the same with the histogram range set to a..b
//     raw              16-bit only: value,count pairs of the non-empty bins of the 65536-bin histogram
//     stackstats       stacks: Stack.getStatistics count, mean, min, max, standard deviation
//     stackcounts      stacks: the 256-bin stack histogram
d = getArgument();
s = "";
kinds = newArray("u8", "u16", "u16skew", "f32", "f32s", "rgb");
for (k=0; k<kinds.length; k++) {
	kind = kinds[k];
	make(kind, 1);
	getStatistics(n, mean, min, max, std);
	s = s + kind + ",stats," + d2s(n, 0) + "," + f(mean) + "," + f(min) + "," + f(max) + "," + f(std) + "," + f(getValue("Mode")) + "\n";
	s = s + hist(kind, 256);
	if (kind=="u8" || kind=="rgb") s = s + hist(kind, 64);
	if (startsWith(kind, "u16")) {
		getHistogram(0, counts, 65536);
		line = kind + ",raw";
		for (i=0; i<65536; i++) if (counts[i]>0) line = line + "," + i + "," + counts[i];
		s = s + line + "\n";
		s = s + hist(kind, 100);
		s = s + histRange(kind, 100, 1000, 30000);
		s = s + histRange(kind, 256, 0, 65535);
	}
	if (startsWith(kind, "f32")) {
		s = s + hist(kind, 40);
		s = s + histRange(kind, 50, -5, 20);
	}
	close();
}
stacks = newArray("u8", "u16", "u16skew", "f32", "f32s", "rgb");
for (k=0; k<stacks.length; k++) {
	kind = stacks[k];
	make(kind, 3);
	Stack.getStatistics(n, mean, min, max, std, counts);
	s = s + kind + ",stackstats," + d2s(n, 0) + "," + f(mean) + "," + f(min) + "," + f(max) + "," + f(std) + "\n";
	s = s + kind + ",stackcounts" + ints(counts) + "\n";
	close();
}
File.saveString(s, d+"/histograms.csv");
print("GEN_OK");

function f(v) { return d2s(v, -12); }

function ints(a) {
	line = "";
	for (i=0; i<a.length; i++) line = line + "," + d2s(a[i], 0);
	return line;
}

function floats(a) {
	line = "";
	for (i=0; i<a.length; i++) line = line + "," + f(a[i]);
	return line;
}

function hist(kind, bins) {
	getHistogram(values, counts, bins);
	key = "" + bins; // a string: a number followed by "+ function()" is parsed as arithmetic
	return kind + ",counts" + key + ints(counts) + "\n" + kind + ",values" + key + floats(values) + "\n";
}

function histRange(kind, bins, a, b) {
	getHistogram(values, counts, bins, a, b);
	key = "" + bins + "_" + a + "_" + b;
	return kind + ",counts" + key + ints(counts) + "\n" + kind + ",values" + key + floats(values) + "\n";
}

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
