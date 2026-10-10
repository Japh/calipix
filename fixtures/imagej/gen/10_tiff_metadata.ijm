// TIFF metadata variants for the TIFF reader, 16x12, beyond what 03-05 cover.
//   16-bit pixel(x,y,c,z,t) = c*10000 + z*1000 + t*100 + x + 16*y      (c,z,t are 1-based)
// Outputs:
//   tiff_composite_color.tif  2 channels x 2 slices, 16-bit, "color" display mode, Green and Magenta LUTs,
//                             display ranges 10000..11000 and 20000..22000
//   tiff_composite_gray.tif   2 channels, 16-bit, "grayscale" display mode, ranges 10000..10500 and 20000..20500
//   tiff_rgb_stack.tif        RGB, 3 slices: r = (x*16) & 255, g = (y*20) & 255, b = s*60   (s is 1-based)
//   tiff_frames_fps.tif       8-bit, 4 frames, v = (t*50 + x + y) mod 256; frame interval 0.5, animation speed 7 fps,
//                             display range 20..180 (so min/max are written for an 8-bit image)
//   tiff_gray16_fire.tif      16-bit single plane v = 100*x + y with the Fire LUT (a ColorMap in a non-palette file),
//                             display range 100..1500
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
W = 16; H = 12;

newImage("tiff_composite_color", "16-bit color-mode", W, H, 2, 2, 1);
for (c=1; c<=2; c++) for (z=1; z<=2; z++) {
	Stack.setPosition(c, z, 1);
	for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, c*10000 + z*1000 + 100 + x + 16*y);
}
Stack.setPosition(1, 1, 1);
Stack.setChannel(1); run("Green");   setMinAndMax(10000, 11000);
Stack.setChannel(2); run("Magenta"); setMinAndMax(20000, 22000);
Stack.setChannel(1);
Stack.getDisplayMode(mode);
print("composite_color mode=" + mode);
saveAs("Tiff", d+"/tiff_composite_color.tif");
close();

newImage("tiff_composite_gray", "16-bit grayscale-mode", W, H, 2, 1, 1);
for (c=1; c<=2; c++) {
	Stack.setChannel(c);
	for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, c*10000 + 1000 + 100 + x + 16*y);
	setMinAndMax(c*10000, c*10000 + 500);
}
Stack.setChannel(1);
Stack.getDisplayMode(mode);
print("composite_gray mode=" + mode);
saveAs("Tiff", d+"/tiff_composite_gray.tif");
close();

newImage("tiff_rgb_stack", "RGB black", W, H, 3);
for (s=1; s<=3; s++) {
	setSlice(s);
	for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, (((x*16)&255)<<16) | (((y*20)&255)<<8) | (s*60));
}
setSlice(1);
saveAs("Tiff", d+"/tiff_rgb_stack.tif");
close();

newImage("tiff_frames_fps", "8-bit black", W, H, 1, 1, 4);
for (t=1; t<=4; t++) {
	Stack.setFrame(t);
	for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, (t*50 + x + y) % 256);
}
Stack.setFrame(1);
Stack.setFrameInterval(0.5);
Stack.setFrameRate(7);
setMinAndMax(20, 180);
Stack.getDimensions(w, h, c, z, t);
print("frames_fps c=" + c + " z=" + z + " t=" + t + " interval=" + Stack.getFrameInterval() + " fps=" + Stack.getFrameRate());
saveAs("Tiff", d+"/tiff_frames_fps.tif");
close();

newImage("tiff_gray16_fire", "16-bit black", W, H, 1);
for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, 100*x + y);
run("Fire");
setMinAndMax(100, 1500);
saveAs("Tiff", d+"/tiff_gray16_fire.tif");
close();
print("GEN_OK");
