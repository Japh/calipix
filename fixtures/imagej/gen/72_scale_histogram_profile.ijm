// References for Analyze>Set Scale, Image>Properties, histograms within a selection and intensity profiles
// (depends on 01_planes.ijm, 04_luts_calibration.ijm and 70_measure.ijm for the masks).
// Outputs:
//   analyze_cases.txt   blocks in the layout of measure_cases.txt ("# name", "@key=value" lines describing the
//                       case, then key=value results). Lists of numbers are separated by spaces.
//     scale_* / properties_*   @command and @options were run on @image (gray8_calibrated.tif first loses its
//                       density calibration), then @roi was measured with every measurement on.
//                       @voxel = pixel width, pixel height, voxel depth, then the unit (getVoxelSize);
//                       @interval = Stack.getFrameInterval; the rest is List.setMeasurements.
//     hist_*            getHistogram(values, counts, @bins) or, with @range, getHistogram(values, counts, @bins,
//                       min, max), and getStatistics, on @image within @roi (@mask as in measure_cases.txt).
//                       Results: count, mean, min, max, std, counts and values (the bin starts).
//     profile_*         getProfile() on @image for @roi: a rectangle (column averages; row averages when
//                       @vertical=1, that is with Alt down) or a line (interpolated samples along it).
var d = "";
var s = "";
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
run("Set Measurements...", "area mean min redirect=None decimal=3");

scale("scale_um", "gray8.tif", "distance=10 known=2.5 pixel=1 unit=um");
scale("scale_aspect", "gray8.tif", "distance=3 known=1 pixel=2 unit=mm");
scale("scale_aspect_zero", "gray8.tif", "distance=7 known=2 pixel=0 unit=inch");
scale("scale_angstrom", "gray8.tif", "distance=64 known=0.5 pixel=1 unit=A");
scale("scale_unit_same", "gray8.tif", "distance=5 known=5 pixel=1 unit=unit");
scale("scale_remove", "gray8_calibrated.tif", "distance=0 known=0 pixel=1 unit=pixel");
scale("scale_no_unit", "gray8_calibrated.tif", "distance=10 known=2 pixel=1 unit=[]");
scale("scale_over_calibrated", "gray8_calibrated.tif", "distance=10 known=2.5 pixel=1.5 unit=cm");

properties("properties_mm", "gray8.tif", "channels=1 slices=1 frames=1 pixel_width=0.5 pixel_height=0.25 voxel_depth=2 unit=mm");
properties("properties_origin", "gray8.tif", "channels=1 slices=1 frames=1 pixel_width=0.2 pixel_height=0.2 voxel_depth=1.5 unit=um frame=[3 sec] origin=3,4");
properties("properties_origin_one", "gray8.tif", "channels=1 slices=1 frames=1 pixel_width=2 pixel_height=3 voxel_depth=1 unit=nm frame=[0.5 min] origin=5");
properties("properties_reset", "gray8_calibrated.tif", "channels=1 slices=1 frames=1 pixel_width=0 pixel_height=7 voxel_depth=7 unit=mm");
properties("properties_none", "gray8_calibrated.tif", "channels=1 slices=1 frames=1 pixel_width=3 pixel_height=3 voxel_depth=3 unit=none");

images = newArray("gray8.tif", "gray16.tif", "gray32f.tif", "rgb.tif");
for (i=0; i<images.length; i++) {
	open(d+"/"+images[i]);
	base = replace(images[i], ".tif", "");
	head = "@image="+images[i]+"\n";
	makeRectangle(8, 6, 20, 10);
	hist("hist_"+base+"_rect", head+"@roi=rect 8 6 20 10\n", 256, 0, 0);
	makeOval(10, 8, 30, 20);
	hist("hist_"+base+"_oval", head+"@roi=oval 10 8 30 20\n@bounds=10 8 30 20\n@mask=measure_mask_oval0.png\n", 256, 0, 0);
	if (bitDepth()==16 || bitDepth()==32) {
		hist("hist_"+base+"_oval_40", head+"@roi=oval 10 8 30 20\n@bounds=10 8 30 20\n@mask=measure_mask_oval0.png\n", 40, 0, 0);
		makeRectangle(0, 20, 64, 3);
		lo = 2000; hi = 30000;
		if (bitDepth()==32) { lo = -5.5; hi = 12.25; }
		hist("hist_"+base+"_range", head+"@roi=rect 0 20 64 3\n", 50, lo, hi);
	}
	run("Select None");
	hist("hist_"+base+"_none", head+"@roi=none\n", 256, 0, 0);

	makeRectangle(8, 6, 20, 10);
	profile("profile_"+base+"_rect", head+"@roi=rect 8 6 20 10\n", 0);
	profile("profile_"+base+"_rect_vertical", head+"@roi=rect 8 6 20 10\n", 1);
	makeRectangle(0, 0, 64, 48);
	profile("profile_"+base+"_all", head+"@roi=rect 0 0 64 48\n", 0);
	makeRectangle(63, 0, 1, 48);
	profile("profile_"+base+"_column", head+"@roi=rect 63 0 1 48\n", 0);
	makeLine(5, 6, 50, 40);
	profile("profile_"+base+"_line", head+"@roi=line 5 6 50 40\n", 0);
	makeLine(10.5, 7.25, 40.75, 30.5);
	profile("profile_"+base+"_line_fraction", head+"@roi=line 10.5 7.25 40.75 30.5\n", 0);
	makeLine(0, 0, 63, 47);
	profile("profile_"+base+"_line_corners", head+"@roi=line 0 0 63 47\n", 0);
	makeLine(63, 10, 0, 10);
	profile("profile_"+base+"_line_back", head+"@roi=line 63 10 0 10\n", 0);
	close();
}

File.saveString(s, d+"/analyze_cases.txt");
print("GEN_OK");

function num(v) { return String.format("%.17g", v); }

function list(a) {
	line = "";
	for (k=0; k<a.length; k++) {
		if (k>0) line = line + " ";
		line = line + num(a[k]);
	}
	return line;
}

function calibrated(name, image, command, options) {
	open(d+"/"+image);
	if (image=="gray8_calibrated.tif") run("Calibrate...", "function=None");
	run(command, options);
	getVoxelSize(pw, ph, pd, unit);
	makeRectangle(8, 6, 20, 10);
	List.setMeasurements;
	s = s + "# "+name+"\n@image="+image+"\n@command="+command+"\n@options="+options+"\n@roi=rect 8 6 20 10\n"
		+ "@voxel="+num(pw)+" "+num(ph)+" "+num(pd)+" "+unit+"\n@interval="+num(Stack.getFrameInterval())+"\n"
		+ List.getList + "\n";
	close();
}

function scale(name, image, options) { calibrated(name, image, "Set Scale...", options); }

function properties(name, image, options) { calibrated(name, image, "Properties...", options); }

function hist(name, head, bins, lo, hi) {
	if (lo==0 && hi==0) {
		getHistogram(values, counts, bins);
		range = "";
	} else {
		getHistogram(values, counts, bins, lo, hi);
		range = "@range="+num(lo)+" "+num(hi)+"\n";
	}
	getStatistics(area, mean, min, max, std);
	total = 0;
	for (k=0; k<counts.length; k++) total = total + counts[k];
	s = s + "# "+name+"\n" + head + "@bins="+bins+"\n" + range
		+ "count="+num(total)+"\nmean="+num(mean)+"\nmin="+num(min)+"\nmax="+num(max)+"\nstd="+num(std)+"\n"
		+ "counts="+list(counts)+"\nvalues="+list(values)+"\n\n";
}

function profile(name, head, vertical) {
	if (vertical==1) setKeyDown("alt");
	p = getProfile();
	setKeyDown("none");
	if (vertical==1) head = head + "@vertical=1\n";
	s = s + "# "+name+"\n" + head + "profile="+list(p)+"\n\n";
}
