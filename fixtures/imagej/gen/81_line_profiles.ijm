// Analyze>Plot Profile along lines of every kind (depends on 01_planes.ijm and 40_roi_shapes.ijm).
// Output:
//   profile_cases.txt   blocks in the layout of measure_cases.txt:
//       # name
//       @image=FILE           the image
//       @cal=PW PH UNIT       pixel size and unit set with Image>Properties (absent: none)
//       @interpolate=0        "Interpolate line profiles" was off in Edit>Options>Plots (absent: on, the default)
//       @roi_file=FILE        the selection, read from a roi_<case>.roi file, or
//       @roi=TEXT             the selection made by the macro, in Calipix's text form
//       profile=V ...         getProfile(): the values of the profile, to 12 significant digits
//       spacing=V             the distance between points in the plot of Analyze>Plot Profile: the X of its
//                             second point (single precision)
var d = "";
var s = "";
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");

files = newArray("line_int", "line_sub", "line_vertical", "line_backwards", "line_cross", "line_wide", "line_wide_horizontal",
	"polyline_int", "polyline_sub", "polyline_wide", "freeline_int", "freeline_sub", "scale_line", "rotate_line_15");
images = newArray("gray8.tif", "gray16.tif", "gray32f.tif", "rgb.tif");
for (i=0; i<images.length; i++) {
	open(d+"/"+images[i]);
	base = replace(images[i], ".tif", "");
	cases(base, "@image="+images[i]+"\n");
	if (i==0) {
		run("Plots...", "width=530 height=300 font=12 draw_ticks minimum=0 maximum=0");
		cases(base+"_nearest", "@image="+images[i]+"\n@interpolate=0\n");
		run("Plots...", "width=530 height=300 font=12 draw_ticks minimum=0 maximum=0 interpolate");
		run("Properties...", "channels=1 slices=1 frames=1 pixel_width=0.5 pixel_height=0.25 voxel_depth=1 unit=mm");
		cases(base+"_cal", "@image="+images[i]+"\n@cal=0.5 0.25 mm\n");
		run("Properties...", "channels=1 slices=1 frames=1 pixel_width=0.3 pixel_height=0.3 voxel_depth=1 unit=um");
		cases(base+"_square", "@image="+images[i]+"\n@cal=0.3 0.3 um\n");
	}
	close();
}

File.saveString(s, d+"/profile_cases.txt");
print("GEN_OK");

function cases(prefix, head) {
	for (k=0; k<files.length; k++) {
		roiManager("reset");
		roiManager("Open", d+"/roi_"+files[k]+".roi");
		roiManager("select", 0);
		record(prefix+"_"+files[k], head+"@roi_file=roi_"+files[k]+".roi\n");
	}
	// Wide lines of other kinds and widths.
	makeLine(5, 5, 50, 30);
	Roi.setStrokeWidth(2.5);
	record(prefix+"_line_w2_5", head+"@roi=line 5 5 50 30 width 2.5\n");
	makeLine(10.5, 7.25, 40.75, 30.5, 3);
	record(prefix+"_line_sub_w3", head+"@roi=line 10.5 7.25 40.75 30.5 width 3\n");
	makeLine(30, 2, 30, 45, 5);
	record(prefix+"_line_vertical_w5", head+"@roi=line 30 2 30 45 width 5\n");
	makeLine(2, 2, 61, 45, 9);
	record(prefix+"_line_edge_w9", head+"@roi=line 2 2 61 45 width 9\n");
	run("Line Width...", "line=1");
	makeSelection("freeline", newArray(5, 9, 15, 22, 30, 36, 44), newArray(40, 30, 24, 22, 26, 34, 38));
	Roi.setStrokeWidth(5);
	record(prefix+"_freeline_w5", head+"@roi=freeline 5,40 9,30 15,24 22,22 30,26 36,34 44,38 width 5\n");
	makeSelection("polyline", newArray(5.5, 20.25, 40.75, 60.1), newArray(5.5, 30.25, 10.5, 40.9));
	Roi.setStrokeWidth(4);
	record(prefix+"_polyline_sub_w4", head+"@roi=polyline 5.5,5.5 20.25,30.25 40.75,10.5 60.1,40.9 width 4\n");
	makeSelection("polyline", newArray(0, 20, 20, 63), newArray(0, 0, 47, 47));
	record(prefix+"_polyline_corners", head+"@roi=polyline 0,0 20,0 20,47 63,47\n");
	makeSelection("polyline", newArray(10, 40), newArray(12, 30));
	record(prefix+"_polyline_two", head+"@roi=polyline 10,12 40,30\n");
	makeRotatedRectangle(10, 10, 50, 35, 12);
	record(prefix+"_rotrect", head+"@roi=rotated-rectangle 10 10 50 35 12\n");
	makeRotatedRectangle(20.5, 40, 40, 5.5, 7.3);
	record(prefix+"_rotrect_sub", head+"@roi=rotated-rectangle 20.5 40 40 5.5 7.3\n");
	run("Select None");
}

function record(name, head) {
	id = getImageID();
	p = getProfile();
	run("Plot Profile");
	Plot.getValues(xs, ys);
	close();
	selectImage(id);
	s = s + "# "+name+"\n" + head + "profile="+list(p)+"\nspacing="+String.format("%.9g", xs[1])+"\n\n";
}

function list(a) {
	line = "";
	for (j=0; j<a.length; j++) {
		if (j>0) line = line + " ";
		line = line + String.format("%.12g", a[j]);
	}
	return line;
}
