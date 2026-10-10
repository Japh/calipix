// 5-D hyperstack: 3 channels x 4 slices x 5 frames, 16-bit, 32x24.
//   pixel(x,y,c,z,t) = c*10000 + z*1000 + t*100 + x + 32*y      (c,z,t are 1-based)
// Calibration: 0.25 x 0.25 x 1.5 um voxels, frame interval 2.5 s.
// Channel LUTs: 1=Cyan 2=Magenta 3=Yellow, composite display mode, distinct display range per channel.
// Also sets an "Info" string and one slice label so tag 50839 carries info/labl/rang/luts blocks.
// Outputs: hyperstack_c3z4t5.tif, stack_z6_8bit.tif (plain 6-slice stack, no hyperstack flag)
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
W = 32; H = 24; C = 3; Z = 4; T = 5;
newImage("hyperstack_c3z4t5", "16-bit composite-mode", W, H, C, Z, T);
for (c=1; c<=C; c++) for (z=1; z<=Z; z++) for (t=1; t<=T; t++) {
	Stack.setPosition(c, z, t);
	base = c*10000 + z*1000 + t*100;
	for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, base + x + 32*y);
}
Stack.setPosition(1, 1, 1);
Stack.setChannel(1); run("Cyan");    setMinAndMax(10000, 15000);
Stack.setChannel(2); run("Magenta"); setMinAndMax(20000, 26000);
Stack.setChannel(3); run("Yellow");  setMinAndMax(30000, 37000);
Stack.setDisplayMode("composite");
setVoxelSize(0.25, 0.25, 1.5, "um");
Stack.setFrameInterval(2.5);
Stack.setPosition(2, 3, 4);
setMetadata("Label", "c2-z3-t4 label");
setMetadata("Info", "fixture=hyperstack_c3z4t5\nkey=value");
Stack.setPosition(1, 1, 1);
Stack.getDimensions(w, h, c, z, t);
getVoxelSize(pw, ph, pd, unit);
print("hyperstack dims " + w + "x" + h + " c=" + c + " z=" + z + " t=" + t + " voxel=" + pw + "," + ph + "," + pd + " interval=" + Stack.getFrameInterval());
saveAs("Tiff", d+"/hyperstack_c3z4t5.tif");
close();

newImage("stack_z6_8bit", "8-bit black", W, H, 6);
for (s=1; s<=6; s++) { setSlice(s); for (y=0; y<H; y++) for (x=0; x<W; x++) setPixel(x, y, (s*40 + x + y) % 256); }
setSlice(1);
saveAs("Tiff", d+"/stack_z6_8bit.tif");
close();
print("GEN_OK");
