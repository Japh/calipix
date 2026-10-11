// Opens one TIFF written by ImageR2 with real ImageJ and prints what ImageJ
// makes of it, in exactly the format of the `.expected.txt` file that the
// ignored test `tiff::tests::write_files_for_imagej_check` writes next to it.
//
// Manual verification, not part of `cargo test`:
//
//   IMAGER_TIFF_CHECK_DIR=<dir> cargo test -p imager-io --lib write_files_for_imagej_check -- --ignored
//   for each <dir>/*.tif:
//     java -Duser.home=<empty dir> -jar ij-1.54p.jar -batch imagej_open_check.ijm <file>  > <file>.actual.txt
//     compare <file>.actual.txt with <file>.expected.txt
//
// ImageJ must run without -Djava.awt.headless=true, and a macro error opens
// a dialog and hangs, so run it under a timeout.
path = getArgument();
open(path);
getDimensions(w, h, c, z, t);
getVoxelSize(pw, ph, pd, unit);
codes = "";
for (i=0; i<lengthOf(unit); i++) codes = codes + charCodeAt(unit, i) + " ";
print("dims " + w + "x" + h + " c=" + c + " z=" + z + " t=" + t + " bits=" + bitDepth() + " n=" + nSlices);
print("voxel " + d2s(pw, 6) + " " + d2s(ph, 6) + " " + d2s(pd, 6) + " unit=" + codes + "interval=" + d2s(Stack.getFrameInterval(), 4));
composite = is("composite");
rgb = bitDepth()==24;
print("flags hyperstack=" + Stack.isHyperstack + " composite=" + composite);
// Statistics are taken inside a selection, so note it and remove it.
selection = selectionType();
if (selection!=-1) run("Select None");
decimals = 0;
if (bitDepth()==32) decimals = 2;
n = 0;
for (ti=1; ti<=t; ti++) for (zi=1; zi<=z; zi++) for (ci=1; ci<=c; ci++) {
	if (nSlices>1) Stack.setPosition(ci, zi, ti);
	if (rgb) {
		v = getPixel(0, 0); a = "" + ((v>>16)&255) + "," + ((v>>8)&255) + "," + (v&255);
		v = getPixel(w-1, h-1); b = "" + ((v>>16)&255) + "," + ((v>>8)&255) + "," + (v&255);
		v = getPixel(5, 3); e = "" + ((v>>16)&255) + "," + ((v>>8)&255) + "," + (v&255);
		print("plane " + n + ": " + a + " " + b + " " + e);
	} else {
		getRawStatistics(count, mean, min, max);
		print("plane " + n + ": " + d2s(getPixel(0, 0), decimals) + " " + d2s(getPixel(w-1, h-1), decimals) + " " + d2s(getPixel(5, 3), decimals)
			+ " min=" + d2s(min, decimals) + " max=" + d2s(max, decimals) + " mean=" + d2s(mean, 3) + " value=" + d2s(calibrate(getPixel(5, 3)), decimals));
	}
	label = getInfo("slice.label");
	if (nSlices>1 && label!="") print("label " + n + "=" + label);
	n++;
}
if (nSlices>1) Stack.setPosition(1, 1, 1);
if (composite) {
	for (ci=1; ci<=c; ci++) {
		Stack.setChannel(ci); getLut(r, g, b); getMinAndMax(lo, hi);
		print("channel " + ci + ": lut " + r[0] + "," + g[0] + "," + b[0] + " " + r[128] + "," + g[128] + "," + b[128] + " " + r[255] + "," + g[255] + "," + b[255]
			+ " range=" + d2s(lo, 4) + ".." + d2s(hi, 4));
	}
	Stack.getDisplayMode(mode); print("mode=" + mode);
} else if (!rgb) {
	getLut(r, g, b); getMinAndMax(lo, hi);
	print("lut " + r[0] + "," + g[0] + "," + b[0] + " " + r[128] + "," + g[128] + "," + b[128] + " " + r[255] + "," + g[255] + "," + b[255]
		+ " range=" + d2s(lo, 4) + ".." + d2s(hi, 4) + " inverting=" + is("Inverting LUT"));
}
print("info=" + replace(getMetadata("Info"), "\n", "|"));
print("property=" + Property.get("Objective"));
print("selection=" + selection + " overlay=" + Overlay.size);
print("CHECK_DONE");
