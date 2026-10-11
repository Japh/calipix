// What ImageJ reads from the hand-built GIF and BMP files in gen/21_crafted_inputs, which hold
// features ImageJ's own writers never produce (GIF disposal methods, transparency, interlacing,
// local colour tables, partial frames; BMP bit depths, palettes and row order).
// The inputs are built by crates/calipix-io/tests/formats_craft.rs.
// Outputs, for each input <name>.<ext>:
//   crafted_<name>_<s>.txt   pixel values of slice s as a text image (8-bit results)
//   crafted_<name>_<s>.png   slice s (RGB results)
//   crafted_<name>_lut.csv   the lookup table, one "r,g,b" line per entry (8-bit results)
//   crafted_info.csv         name,bitDepth,slices of every input
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);

// The inputs sit beside this macro; the output directory is either fixtures/imagej/out (gen)
// or target/verify-out (verify).
src = File.getParent(d)+"/gen/21_crafted_inputs/";
if (!File.exists(src+"dispose.gif"))
	src = File.getParent(File.getParent(d))+"/fixtures/imagej/gen/21_crafted_inputs/";
if (!File.exists(src+"dispose.gif")) {
	print("cannot find 21_crafted_inputs from "+d);
	exit;
}

names = newArray("dispose.gif", "gray_first.gif", "single.gif", "single_full.gif", "single_trans.gif",
	"single_local.gif", "single_part.gif", "bits1.bmp", "bits4.bmp", "bits8_topdown.bmp",
	"bits24_topdown.bmp", "bits32.bmp");
info = "";
for (i=0; i<names.length; i++) {
	open(src+names[i]);
	base = "crafted_"+substring(names[i], 0, lengthOf(names[i])-4);
	n = nSlices;
	depth = bitDepth();
	info = info+names[i]+","+depth+","+n+"\n";
	if (depth==8) {
		getLut(r, g, b);
		s = "";
		for (k=0; k<r.length; k++) s = s+r[k]+","+g[k]+","+b[k]+"\n";
		File.saveString(s, d+"/"+base+"_lut.csv");
	}
	id = getImageID();
	for (s=1; s<=n; s++) {
		selectImage(id);
		setSlice(s);
		run("Duplicate...", "title=slice");
		if (depth==8)
			saveAs("Text Image", d+"/"+base+"_"+s+".txt");
		else
			saveAs("PNG", d+"/"+base+"_"+s+".png");
		close();
	}
	selectImage(id);
	close();
}
File.saveString(info, d+"/crafted_info.csv");
print("GEN_OK");
