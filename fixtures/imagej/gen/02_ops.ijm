// Golden results of basic operations applied to the base planes written by 01_planes.ijm.
// (Depends on 01: the driver runs gen/*.ijm in lexical order.)
// Outputs: <base>_invert.tif, <base>_gauss2.tif for base in gray8, gray16, gray32f, rgb
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
bases = newArray("gray8", "gray16", "gray32f", "rgb");
for (i=0; i<bases.length; i++) {
	b = bases[i];
	open(d+"/"+b+".tif");
	run("Invert");
	saveAs("Tiff", d+"/"+b+"_invert.tif");
	close();
	open(d+"/"+b+".tif");
	run("Gaussian Blur...", "sigma=2");
	saveAs("Tiff", d+"/"+b+"_gauss2.tif");
	close();
}
print("GEN_OK");
