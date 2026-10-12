// Selections of every kind on a 64x48 image, and what ImageJ makes of them.
// Outputs, one entry per case in the order the cases appear below:
//   roi_<case>.roi   the selection as ImageJ saves it
//   roi_info.csv     type, bounds, vertex count and measurements (uncalibrated, then with 0.5 x 0.25 um pixels)
//   roi_coords.csv   getSelectionCoordinates, one vertex per line
//   roi_masks.csv    the pixels filling the selection sets, 48 rows of 16 hex digits (64 pixels, leftmost first) per case
// Areas and points are filled with the macro function fill(), which uses Roi.getMask(). Lines are filled with
// Edit > Fill, which draws a line (Roi.drawPixels) or, for a wide straight line, fills the area it covers.
var d, n, info, coords, masks;

function hexMask() {
	hex = "0123456789abcdef";
	s = "";
	for (y = 0; y < 48; y++) {
		s = s + ",";
		for (x = 0; x < 64; x += 4) {
			v = 0;
			if (getPixel(x, y) > 0) v += 8;
			if (getPixel(x + 1, y) > 0) v += 4;
			if (getPixel(x + 2, y) > 0) v += 2;
			if (getPixel(x + 3, y) > 0) v += 1;
			s = s + substring(hex, v, v + 1);
		}
	}
	return s;
}

// Records the current selection as case `name`, then clears the image and the selection.
function rec(name) {
	n++;
	type = selectionType();
	getSelectionBounds(bx, by, bw, bh);
	Roi.getFloatBounds(fx, fy, fw, fh);
	row = name + "," + type + "," + bx + "," + by + "," + bw + "," + bh;
	row = row + "," + d2s(fx, 9) + "," + d2s(fy, 9) + "," + d2s(fw, 9) + "," + d2s(fh, 9);
	row = row + "," + getValue("selection.size");
	keys = newArray("Area", "X", "Y", "Perim.", "BX", "BY", "Width", "Height", "Major", "Minor", "Angle", "Feret", "FeretX", "FeretY", "FeretAngle", "MinFeret", "Length");
	for (i = 0; i < keys.length; i++)
		row = row + "," + d2s(getValue(keys[i]), 9);
	setVoxelSize(0.5, 0.25, 1, "um");
	keys = newArray("Area", "Perim.", "Length", "Feret", "FeretX", "FeretY", "FeretAngle", "MinFeret");
	for (i = 0; i < keys.length; i++)
		row = row + "," + d2s(getValue(keys[i]), 9);
	setVoxelSize(1, 1, 1, "pixel");
	info = info + row + "\n";
	getSelectionCoordinates(xs, ys);
	for (i = 0; i < xs.length; i++)
		coords = coords + name + "," + i + "," + d2s(xs[i], 5) + "," + d2s(ys[i], 5) + "\n";
	saveAs("Selection", d + "/roi_" + name + ".roi");
	if (type >= 5 && type <= 8) {
		setColor(255);
		run("Fill", "slice");
	} else {
		setColor(255);
		fill();
	}
	masks = masks + name + hexMask() + "\n";
	run("Select All");
	setColor(0);
	fill();
	run("Select None");
}

d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
newImage("roi-host", "8-bit black", 64, 48, 1);
n = 0;
info = "name,type,bx,by,bw,bh,fx,fy,fw,fh,size,Area,X,Y,Perim,BX,BY,Width,Height,Major,Minor,Angle,Feret,FeretX,FeretY,FeretAngle,MinFeret,Length,cArea,cPerim,cLength,cFeret,cFeretX,cFeretY,cFeretAngle,cMinFeret\n";
coords = "name,index,x,y\n";
masks = "";

// Rectangles
makeRectangle(10, 8, 20, 12); rec("rect_int");
makeRectangle(10.5, 8.25, 20.3, 12.6); rec("rect_sub");
makeRectangle(5, 5, 1, 1); rec("rect_tiny");
makeRectangle(-5, -4, 20, 12); rec("rect_cross_tl");
makeRectangle(50, 40, 30, 20); rec("rect_cross_br");
makeRectangle(0, 0, 64, 48); rec("rect_all");
makeRectangle(10, 8, 30, 20, 10); rec("rounded");
makeRectangle(3, 3, 7, 5, 4); rec("rounded_small");
makeRectangle(10, 10, 20, 10, 30); rec("rounded_big_arc");
makeRectangle(40, 30, 40, 30, 16); rec("rounded_cross");
makeRectangle(2, 2, 60, 44, 23); rec("rounded_large");

// Ovals
makeOval(20, 10, 15, 20); rec("oval_int");
makeOval(5, 5, 21, 21); rec("oval_circle_odd");
makeOval(8, 6, 30, 30); rec("oval_circle_even");
makeOval(10, 10, 40, 3); rec("oval_thin");
makeOval(3, 3, 2, 2); rec("oval_2");
makeOval(3, 3, 1, 1); rec("oval_1");
makeOval(3, 3, 5, 4); rec("oval_small");
makeOval(10.5, 8.25, 20.3, 12.6); rec("oval_sub");
makeOval(-10, -8, 30, 25); rec("oval_cross_tl");
makeOval(45, 30, 40, 40); rec("oval_cross_br");
makeOval(0, 0, 64, 48); rec("oval_all");

// Polygons with whole-number vertices
makePolygon(40, 5, 60, 10, 50, 40); rec("poly_tri");
makePolygon(5, 5, 30, 8, 18, 20, 32, 40, 4, 30, 12, 18); rec("poly_concave");
makePolygon(10, 10, 50, 40, 50, 10, 10, 40); rec("poly_bowtie");
makePolygon(32, 4, 44, 42, 12, 18, 52, 18, 20, 42); rec("poly_star");
makePolygon(-10, 5, 30, -8, 70, 30, 20, 60); rec("poly_cross");
makePolygon(5, 5, 60, 6, 5, 7); rec("poly_thin");
makePolygon(5, 5, 25, 5, 25, 15, 15, 15, 15, 30, 5, 30); rec("poly_rectilinear");
makePolygon(10, 10, 14, 10, 14, 13, 10, 13); rec("poly_small_box");
makePolygon(3, 3, 5, 3, 4, 5); rec("poly_tiny");
makePolygon(0, 0, 64, 0, 64, 48, 0, 48); rec("poly_all");
makePolygon(8, 40, 20, 4, 56, 4, 44, 40); rec("poly_parallelogram");

// Polygons with fractional vertices
makeSelection("polygon", newArray(10.5, 40.75, 22.3), newArray(5.25, 8.5, 30.9)); rec("polyf_tri");
makeSelection("polygon", newArray(-6.5, 30.25, 70.4, 20.6), newArray(5.5, -8.25, 30.7, 60.1)); rec("polyf_cross");
makeSelection("polygon", newArray(3.2, 5.9, 4.1), newArray(3.2, 3.4, 6.8)); rec("polyf_small");
makeSelection("polygon", newArray(10.3, 50.6, 50.1, 10.9), newArray(10.2, 40.7, 10.4, 40.3)); rec("polyf_bowtie");
makeSelection("polygon", newArray(10.5, 20.5, 20.5, 10.5), newArray(10.5, 10.5, 20.5, 20.5)); rec("polyf_half_box");
makeSelection("polygon", newArray(10, 20.5, 20, 10), newArray(10, 10, 20, 20)); rec("polyf_mixed");

// Freehand and traced
makeSelection("freehand", newArray(10, 14, 22, 30, 36, 34, 26, 15, 8), newArray(12, 6, 4, 8, 16, 28, 36, 34, 22)); rec("free_int");
makeSelection("freehand", newArray(10.5, 14.2, 22.8, 30.1, 36.6, 34.3, 26.9, 15.4, 8.7), newArray(12.1, 6.6, 4.3, 8.8, 16.2, 28.5, 36.4, 34.9, 22.3)); rec("free_sub");
makeSelection("traced", newArray(5, 9, 9, 12, 12, 8, 8, 5), newArray(5, 5, 7, 7, 12, 12, 10, 10)); rec("traced_int");
makeSelection("traced", newArray(20, 21, 21, 22, 22, 23, 23, 20), newArray(20, 20, 21, 21, 22, 22, 23, 23)); rec("traced_stairs");
makeSelection("traced", newArray(30, 31, 31, 30), newArray(30, 30, 31, 31)); rec("traced_1px");

// Straight lines
makeLine(2, 40, 60, 44); rec("line_int");
makeLine(2.5, 3.25, 40.75, 30.5); rec("line_sub");
makeLine(10, 2, 10, 40); rec("line_vertical");
makeLine(50, 30, 12, 6); rec("line_backwards");
makeLine(-5, 10, 70, 20); rec("line_cross");
makeLine(5, 5, 50, 30, 7); rec("line_wide");
makeLine(8, 40, 56, 40, 4); rec("line_wide_horizontal");

// Segmented and freehand lines, angles
makeLine(5, 5, 20, 30, 40, 10, 60, 40); rec("polyline_int");
makeSelection("polyline", newArray(5.5, 20.25, 40.75, 60.1), newArray(5.5, 30.25, 10.5, 40.9)); rec("polyline_sub");
makeLine(5, 5, 20, 30, 40, 10, 60, 40); Roi.setStrokeWidth(6); rec("polyline_wide");
makeSelection("freeline", newArray(5, 9, 15, 22, 30, 36, 44), newArray(40, 30, 24, 22, 26, 34, 38)); rec("freeline_int");
makeSelection("freeline", newArray(5.5, 9.2, 15.8, 22.1, 30.6, 36.3, 44.9), newArray(40.1, 30.6, 24.3, 22.8, 26.2, 34.5, 38.4)); rec("freeline_sub");
makeSelection("angle", newArray(10, 30, 50), newArray(40, 10, 35)); rec("angle_int");
makeSelection("angle", newArray(10.5, 30.25, 50.75), newArray(40.5, 10.25, 35.5)); rec("angle_sub");

// Points
makePoint(33, 17); rec("point_int");
makePoint(10.5, 20.25); rec("point_sub");
makeSelection("point", newArray(5, 20, 40, 63), newArray(5, 30, 12, 47)); rec("points_int");
makeSelection("point", newArray(5.5, 20.25, 40.75, 62.4), newArray(5.5, 30.25, 12.75, 46.6)); rec("points_sub");
makeSelection("point", newArray(-3, 20, 70), newArray(5, 30, 50)); rec("points_cross");

// Ellipses and rotated rectangles
makeEllipse(10, 10, 50, 35, 0.5); rec("ellipse");
makeEllipse(20.5, 40.2, 40.1, 5.7, 0.3); rec("ellipse_sub");
makeEllipse(10, 24, 50, 24, 1.0); rec("ellipse_circle");
makeEllipse(-10, 10, 40, 60, 0.6); rec("ellipse_cross");
makeEllipse(30, 20, 34, 23, 0.8); rec("ellipse_small");
makeRotatedRectangle(10, 10, 50, 35, 12); rec("rotrect");
makeRotatedRectangle(20.5, 40, 40, 5.5, 7.3); rec("rotrect_sub");
makeRotatedRectangle(10, 24, 50, 24, 10); rec("rotrect_axis");
makeRotatedRectangle(-10, 10, 40, 60, 15); rec("rotrect_cross");

// Composites
makeRectangle(5, 5, 20, 15); setKeyDown("shift"); makeOval(15, 10, 25, 25); setKeyDown("none"); rec("comp_or");
makeRectangle(5, 5, 40, 30); setKeyDown("alt"); makeOval(15, 10, 20, 15); setKeyDown("none"); rec("comp_hole");
makeRectangle(2, 2, 12, 10); setKeyDown("shift"); makeRectangle(30, 20, 20, 15); setKeyDown("none"); rec("comp_two");
makePolygon(5, 5, 40, 8, 20, 40); setKeyDown("shift"); makePolygon(30, 30, 60, 20, 55, 45); setKeyDown("none"); rec("comp_or_polys");
makeSelection("polygon", newArray(10.5, 40.75, 22.3), newArray(5.25, 8.5, 30.9)); setKeyDown("alt"); makeRectangle(18, 10, 8, 8); setKeyDown("none"); rec("comp_sub_minus_rect");
makeRectangle(10, 8, 30, 20, 10); setKeyDown("shift"); makeRectangle(30, 20, 30, 20, 12); setKeyDown("none"); rec("comp_curves");
makeRectangle(5, 5, 30, 20); roiManager("Add"); makeOval(20, 12, 30, 25); roiManager("Add");
roiManager("select", newArray(0, 1)); roiManager("AND"); rec("comp_and");
roiManager("select", newArray(0, 1)); roiManager("XOR"); rec("comp_xor");
roiManager("select", newArray(0, 1)); roiManager("Combine"); rec("comp_combine");
roiManager("reset");
makeOval(10, 8, 25, 20); run("Make Inverse"); rec("inverse_oval");
makeRectangle(10, 8, 25, 20); run("Make Inverse"); rec("inverse_rect");
makeRectangle(0, 0, 20, 48); run("Make Inverse"); rec("inverse_rect_side");
makeRectangle(0, 0, 20, 30); run("Make Inverse"); rec("inverse_rect_corner");
makeSelection("polygon", newArray(10.5, 40.75, 22.3), newArray(5.25, 8.5, 30.9)); run("Make Inverse"); rec("inverse_polyf");
makePolygon(-10, 5, 30, -8, 70, 30, 20, 60); run("Make Inverse"); rec("inverse_poly_cross");
makeRectangle(10, 8, 30, 20, 10); run("Make Inverse"); rec("inverse_rounded");
makePolygon(40, 5, 60, 10, 50, 40); run("Make Inverse"); rec("inverse_poly_tri");
makePolygon(8, 40, 20, 4, 56, 4, 44, 40); run("Make Inverse"); rec("inverse_parallelogram");
makePolygon(10, 10, 50, 40, 50, 10, 10, 40); run("Make Inverse"); rec("inverse_bowtie");
makeOval(45, 30, 40, 40); run("Make Inverse"); rec("inverse_oval_cross");
makeRectangle(5, 5, 40, 30); setKeyDown("alt"); makeOval(15, 10, 20, 15); setKeyDown("none"); run("Make Inverse"); rec("inverse_hole");
makeEllipse(10, 10, 50, 35, 0.5); run("Make Inverse"); rec("inverse_ellipse");
// A thresholded shape with a hole, a diagonal neighbour and a separate blob.
setColor(255);
makeOval(6, 6, 24, 20); fill();
makeRectangle(40, 10, 10, 10); fill();
makeRectangle(50, 20, 6, 6); fill();
makeRectangle(44, 36, 1, 1); fill();
setColor(0);
makeRectangle(14, 12, 6, 5); fill();
run("Select None");
setThreshold(255, 255);
run("Create Selection");
resetThreshold();
roiManager("Add");
run("Select All"); setColor(0); fill(); run("Select None");
roiManager("select", 0); rec("comp_threshold");
roiManager("reset");

// Selections made from others
makePolygon(5, 5, 30, 8, 18, 20, 32, 40, 4, 30, 12, 18); run("Enlarge...", "enlarge=3"); rec("enlarge_poly_3");
makePolygon(5, 5, 30, 8, 18, 20, 32, 40, 4, 30, 12, 18); run("Enlarge...", "enlarge=-2"); rec("enlarge_poly_m2");
makeOval(20, 10, 15, 20); run("Enlarge...", "enlarge=4"); rec("enlarge_oval_4");
makeRectangle(10, 8, 20, 12); run("Enlarge...", "enlarge=-3"); rec("enlarge_rect_m3");
makeSelection("polygon", newArray(10.5, 40.75, 22.3), newArray(5.25, 8.5, 30.9)); run("Enlarge...", "enlarge=5"); rec("enlarge_polyf_5");
makeRectangle(5, 5, 40, 30); setKeyDown("alt"); makeOval(15, 10, 20, 15); setKeyDown("none"); run("Enlarge...", "enlarge=2"); rec("enlarge_hole_2");
makeEllipse(10, 10, 50, 35, 0.5); run("Enlarge...", "enlarge=-4"); rec("enlarge_ellipse_m4");
makePolygon(40, 5, 60, 10, 50, 40); run("Scale... ", "x=0.5 y=0.75"); rec("scale_poly");
makePolygon(40, 5, 60, 10, 50, 40); run("Scale... ", "x=0.5 y=0.75 centered"); rec("scale_poly_centered");
makeOval(20, 10, 15, 20); run("Scale... ", "x=1.5 y=1.25 centered"); rec("scale_oval_centered");
makeRectangle(10, 8, 20, 12); run("Scale... ", "x=1.5 y=0.5"); rec("scale_rect");
makeLine(2, 40, 60, 44); run("Scale... ", "x=0.5 y=0.5"); rec("scale_line");
makeLine(2, 40, 60, 44); run("Scale... ", "x=0.5 y=0.5 centered"); rec("scale_line_centered");
makeRectangle(5, 5, 20, 15); setKeyDown("shift"); makeOval(15, 10, 25, 25); setKeyDown("none"); run("Scale... ", "x=0.75 y=0.5"); rec("scale_comp");
makeRectangle(5, 5, 20, 15); setKeyDown("shift"); makeOval(15, 10, 25, 25); setKeyDown("none"); run("Scale... ", "x=0.75 y=0.5 centered"); rec("scale_comp_centered");
makeRectangle(5, 5, 40, 30); setKeyDown("alt"); makeOval(15, 10, 20, 15); setKeyDown("none"); run("Scale... ", "x=0.75 y=0.5"); rec("scale_hole");
makeRectangle(5, 5, 40, 30); setKeyDown("alt"); makeOval(15, 10, 20, 15); setKeyDown("none"); run("Scale... ", "x=0.75 y=0.5 centered"); rec("scale_hole_centered");
makeRectangle(5, 5, 40, 30); setKeyDown("alt"); makeOval(15, 10, 20, 15); setKeyDown("none"); run("Rotate...", "  angle=25"); rec("rotate_hole_25");
makeRectangle(20, 14, 24, 12); run("Rotate...", "  angle=30"); rec("rotate_rect_30");
makeOval(20, 10, 15, 20); run("Rotate...", "  angle=-45"); rec("rotate_oval_m45");
makePolygon(40, 5, 60, 10, 50, 40); run("Rotate...", "  angle=90"); rec("rotate_poly_90");
makeLine(2, 40, 60, 44); run("Rotate...", "  angle=15"); rec("rotate_line_15");
makeRotatedRectangle(10, 10, 50, 35, 12); run("Rotate...", "  angle=20"); rec("rotate_rotrect_20");
makeRectangle(5, 5, 20, 15); setKeyDown("shift"); makeOval(15, 10, 25, 25); setKeyDown("none"); run("Rotate...", "  angle=25"); rec("rotate_comp_25");
makeSelection("point", newArray(5, 20, 40, 60), newArray(5, 30, 12, 40)); run("Rotate...", "  angle=10"); rec("rotate_points_10");
makePolygon(5, 5, 30, 8, 18, 20, 32, 40, 4, 30, 12, 18); run("Fit Ellipse"); rec("fit_ellipse_poly");
makeRectangle(10, 8, 20, 12); run("Fit Ellipse"); rec("fit_ellipse_rect");
makeOval(10, 10, 40, 13); run("Fit Ellipse"); rec("fit_ellipse_oval");
makeRectangle(5, 5, 20, 15); setKeyDown("shift"); makeOval(15, 10, 25, 25); setKeyDown("none"); run("Fit Ellipse"); rec("fit_ellipse_comp");
makePolygon(5, 5, 30, 8, 18, 20, 32, 40, 4, 30, 12, 18); run("Convex Hull"); rec("hull_poly");
makeOval(20, 10, 15, 20); run("Convex Hull"); rec("hull_oval");
makeSelection("point", newArray(5, 20, 40, 60, 30), newArray(5, 30, 12, 40, 20)); run("Convex Hull"); rec("hull_points");
makeRectangle(5, 5, 40, 30); setKeyDown("alt"); makeOval(15, 10, 20, 15); setKeyDown("none"); run("Convex Hull"); rec("hull_comp");
makeOval(20.5, 10.25, 15.3, 20.6); run("To Bounding Box"); rec("box_oval_sub");
makeSelection("polygon", newArray(10.5, 40.75, 22.3), newArray(5.25, 8.5, 30.9)); run("To Bounding Box"); rec("box_polyf");
makeLine(2, 40, 60, 44); run("To Bounding Box"); rec("box_line");
makeLine(5, 5, 50, 30, 7); run("Line to Area"); rec("area_line_wide");
makeLine(2, 40, 60, 44); run("Line to Area"); rec("area_line_thin");
makeLine(5, 5, 20, 30, 40, 10, 60, 40); Roi.setStrokeWidth(6); run("Line to Area"); rec("area_polyline_wide");
makeLine(5, 5, 20, 30, 40, 10, 60, 40); run("Line to Area"); rec("area_polyline_thin");
makeSelection("freeline", newArray(5.5, 9.2, 15.8, 22.1, 30.6, 36.3, 44.9), newArray(40.1, 30.6, 24.3, 22.8, 26.2, 34.5, 38.4)); Roi.setStrokeWidth(3); run("Line to Area"); rec("area_freeline_sub");

// Properties ImageJ stores with a selection
makeOval(20, 10, 15, 20); Roi.setStrokeColor("red"); Roi.setStrokeWidth(2.5); setSelectionName("nucleus"); rec("props_stroke");
makeRectangle(10, 8, 20, 12); Roi.setFillColor("#4000ff00"); rec("props_fill");
makePolygon(40, 5, 60, 10, 50, 40); Roi.setGroup(7); Roi.setProperty("kind", "test"); Roi.setPosition(2); rec("props_group");
makeLine(2, 40, 60, 44); Roi.setStrokeColor("#8000ffff"); Roi.setPosition(2, 3, 4); rec("props_line");

File.saveString(info, d + "/roi_info.csv");
File.saveString(coords, d + "/roi_coords.csv");
File.saveString(masks, d + "/roi_masks.csv");
print("cases=" + n);
print("GEN_OK");
