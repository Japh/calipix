// Single-plane base images, 64x48, deterministic arithmetic patterns.
// The patterns are defined so the Rust side can regenerate them exactly:
//   gray8  : v = (x*x + 3*y*y + x*y) mod 256
//   gray16 : v = (x*257 + y*y*31 + 1000*((floor(x/8)+floor(y/8)) mod 2)) mod 65536
//   gray32f: v = (float)((x-31.5)*0.25 + (y*y)/64.0 - 3.5*((x+y) mod 5))     (has negatives)
//   rgb    : r = (x*4) & 255, g = (y*5) & 255, b = ((x+y)*3) & 255
// Outputs: gray8.tif gray16.tif gray32f.tif rgb.tif gray8.png rgb.png gray16.png gray8_text.txt gray32f_text.txt
d = getArgument();
W = 64; H = 48;
// pin every preference that influences output (do not rely on IJ_Prefs.txt)
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row"); // no "save" keyword => Motorola (big-endian) TIFF
setOption("BlackBackground", true);

newImage("gray8", "8-bit black", W, H, 1);
for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, (x*x + 3*y*y + x*y) % 256);
updateDisplay();
saveAs("Tiff", d+"/gray8.tif");
saveAs("PNG", d+"/gray8.png");
saveAs("Text Image", d+"/gray8_text.txt");
close();

newImage("gray16", "16-bit black", W, H, 1);
for (y=0; y<H; y++) for (x=0; x<W; x++)
	setPixel(x, y, (x*257 + y*y*31 + 1000*((floor(x/8)+floor(y/8))%2)) % 65536);
resetMinAndMax();
saveAs("Tiff", d+"/gray16.tif");
saveAs("PNG", d+"/gray16.png"); // 16-bit PNG
close();

newImage("gray32f", "32-bit black", W, H, 1);
for (y=0; y<H; y++) for (x=0; x<W; x++)
	setPixel(x, y, (x-31.5)*0.25 + (y*y)/64.0 - 3.5*((x+y)%5));
resetMinAndMax();
saveAs("Tiff", d+"/gray32f.tif");
saveAs("Text Image", d+"/gray32f_text.txt");
close();

newImage("rgb", "RGB black", W, H, 1);
for (y=0; y<H; y++) for (x=0; x<W; x++) {
	r = (x*4)&255; g = (y*5)&255; b = ((x+y)*3)&255;
	setPixel(x, y, (r<<16) | (g<<8) | b);
}
updateDisplay();
saveAs("Tiff", d+"/rgb.tif");
saveAs("PNG", d+"/rgb.png");
close();

print("GEN_OK");
