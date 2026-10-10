// Measurement goldens (depends on 01_planes.ijm).
// Outputs:
//   measure_rect_<base>.csv  Analyze>Measure on rectangle (8,6,20,10) of gray8, gray16, gray32f (one row each)
//   measure_oval_<base>.csv  same measurements on oval (10,8,30,20)
//   gray8_otsu_mask.tif      setAutoThreshold("Otsu dark") + Convert to Mask
//   blobs_particles.csv      Analyze Particles results on a synthetic binary image
//   blobs.tif                the synthetic binary image
//   blobs_count_masks.tif    Analyze Particles "Count Masks" output (16-bit label image)
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
run("Set Measurements...", "area mean standard modal min centroid center perimeter bounding fit shape feret's integrated median skewness kurtosis display redirect=None decimal=6");
bases = newArray("gray8", "gray16", "gray32f");

// NB: without a Results window (-batch) every run("Measure") replaces the table instead of appending a row,
// so each measurement is saved to its own one-row CSV.
for (i=0; i<bases.length; i++) {
	open(d+"/"+bases[i]+".tif");
	makeRectangle(8, 6, 20, 10); run("Measure");
	if (nResults!=1) exit("unexpected nResults "+nResults);
	saveAs("Results", d+"/measure_rect_"+bases[i]+".csv");
	makeOval(10, 8, 30, 20); run("Measure");
	saveAs("Results", d+"/measure_oval_"+bases[i]+".csv");
	close();
}
run("Clear Results");

open(d+"/gray8.tif");
setAutoThreshold("Otsu dark");
getThreshold(lo, hi); print("otsu threshold " + lo + " " + hi);
run("Convert to Mask");
saveAs("Tiff", d+"/gray8_otsu_mask.tif");
close();

newImage("blobs", "8-bit black", 64, 48, 1);
setColor(255);
fillRect(4, 4, 10, 8); fillOval(22, 6, 15, 12); fillOval(44, 20, 14, 22); fillRect(6, 30, 20, 3); fillOval(30, 30, 9, 9);
setColor(0); fillOval(47, 26, 6, 6); // a hole
saveAs("Tiff", d+"/blobs.tif");
setThreshold(128, 255);
run("Set Measurements...", "area mean centroid perimeter bounding shape redirect=None decimal=6");
run("Analyze Particles...", "size=0-Infinity show=[Count Masks] display clear");
print("particles=" + nResults);
saveAs("Results", d+"/blobs_particles.csv");
selectImage("Count Masks of blobs.tif");
saveAs("Tiff", d+"/blobs_count_masks.tif");
print("GEN_OK");
