// Little-endian ("Intel byte order") saves of the base planes and of a small stack (depends on 01_planes.ijm).
// ImageJ's default is big-endian (Motorola); the "save" keyword of Edit>Options>Input/Output flips it.
// Outputs: gray16_le.tif gray32f_le.tif rgb_le.tif stack_z3_16bit_le.tif
d = getArgument();
setOption("BlackBackground", true);
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save save_column save_row"); // "save" => Intel byte order
bases = newArray("gray16", "gray32f", "rgb");
for (i=0; i<bases.length; i++) {
	open(d+"/"+bases[i]+".tif");
	saveAs("Tiff", d+"/"+bases[i]+"_le.tif");
	close();
}
newImage("stack_z3_16bit_le", "16-bit black", 32, 24, 3);
for (s=1; s<=3; s++) { setSlice(s); for (y=0; y<24; y++) for (x=0; x<32; x++) setPixel(x, y, s*1000 + x + 32*y); }
setSlice(1); resetMinAndMax();
setVoxelSize(0.1, 0.1, 0.5, "um");
saveAs("Tiff", d+"/stack_z3_16bit_le.tif");
close();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
print("GEN_OK");
