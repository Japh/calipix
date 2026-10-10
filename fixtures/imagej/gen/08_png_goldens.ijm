// Inverted planes saved as PNG, for checking operations before TIFF reading exists.
// (Depends on 01: the driver runs gen/*.ijm in lexical order.)
// Outputs: <base>_invert.png for base in gray8, gray16, rgb
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
bases = newArray("gray8", "gray16", "rgb");
for (i=0; i<bases.length; i++) {
	b = bases[i];
	open(d+"/"+b+".tif");
	run("Invert");
	saveAs("PNG", d+"/"+b+"_invert.png");
	close();
}
print("GEN_OK");
