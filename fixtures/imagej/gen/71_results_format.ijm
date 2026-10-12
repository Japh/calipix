// The Results table as text: which columns Analyze>Measure writes for which settings, and how numbers are
// formatted (depends on 01_planes.ijm, 03_hyperstack.ijm and 04_luts_calibration.ijm).
// Outputs:
//   results_cases.txt      one line per measured table: file, image, position (C Z T or -), selection and the
//                          Set Measurements options, separated by tabs
//   results_<name>.csv     the table saved by File>Save As>Results after one Analyze>Measure (.txt: tab-separated)
//   results_numbers*.csv   a table of awkward numbers and text built with setResult, at 3 decimal places
//   results_numbers.txt    the same, tab-separated
//   results_numbers_sci5.csv   the same after choosing scientific notation with 5 decimal places
//   results_numbers_dec0.csv   the same at 0 decimal places
//   results_rows.csv       a measured row followed by rows built with setResult, in scientific notation
//   results_nan_cells.csv  a table with unfilled cells while "NaN empty cells" is on
var d = "";
var index = "";
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
ALL = "area mean standard modal min centroid center perimeter bounding fit shape feret's integrated median skewness kurtosis area_fraction stack";

one("default.csv", "gray8.tif", "-", "none", "area mean min redirect=None decimal=3");
one("all_label.csv", "gray32f.tif", "-", "rect 8 6 20 10", ALL+" display redirect=None decimal=3");
one("all_label.txt", "gray8.tif", "-", "rect 8 6 20 10", ALL+" display redirect=None decimal=3");
one("all_sci4.csv", "gray16.tif", "-", "rect 44 30 20 18", ALL+" scientific redirect=None decimal=4");
one("all_dec0.csv", "gray8.tif", "-", "rect 8 6 20 10", ALL+" redirect=None decimal=0");
one("all_dec9.csv", "gray32f.tif", "-", "none", ALL+" redirect=None decimal=9");
one("all_rgb.csv", "rgb.tif", "-", "rect 0 20 64 3", ALL+" display redirect=None decimal=3");
one("shape_feret.csv", "rgb.tif", "-", "rect 5 9 13 1", "shape feret's redirect=None decimal=2");
one("one_pixel.csv", "gray16.tif", "-", "rect 63 47 1 1", ALL+" redirect=None decimal=3");
one("hyper.csv", "hyperstack_c3z4t5.tif", "2 3 4", "rect 3 2 10 7", "area mean min stack display redirect=None decimal=3");
one("hyper_all.csv", "hyperstack_c3z4t5.tif", "3 1 5", "none", ALL+" redirect=None decimal=3");
one("calibrated.csv", "gray8_calibrated.tif", "-", "rect 8 6 20 10", ALL+" display redirect=None decimal=3");
one("inverty.csv", "gray8_calibrated.tif", "-", "rect 8 6 20 10", "area centroid center bounding invert redirect=None decimal=4");
one("nan_option.csv", "gray8.tif", "-", "rect 8 6 20 10", "area mean min nan redirect=None decimal=3");
one("line.csv", "gray8.tif", "-", "line 5 6 50 40", "area mean min redirect=None decimal=3");
one("line_all.csv", "gray16.tif", "-", "line 10.5 7.25 40.75 30.5", ALL+" display redirect=None decimal=3");
one("line_length.csv", "gray8.tif", "-", "line 5 6 50 40", "perimeter display redirect=None decimal=3");
one("line_calibrated.csv", "gray8_calibrated.tif", "-", "line 2 10 61 10", "area mean centroid bounding redirect=None decimal=3");
one("point.csv", "gray8.tif", "-", "point 12 20", "area mean min redirect=None decimal=3");
one("point_all.csv", "gray8_calibrated.tif", "-", "point 31 5", ALL+" display redirect=None decimal=3");
one("point_hyper.csv", "hyperstack_c3z4t5.tif", "2 3 4", "point 4 5", "mean redirect=None decimal=3");
File.saveString(index, d+"/results_cases.txt");

// Numbers and text that exercise ResultsTable.d2s and the CSV quoting. Column A gets decimals because it holds
// fractions; B holds whole numbers only; C holds whole numbers too large for an int; S holds text.
run("Set Measurements...", "area mean min redirect=None decimal=3");
run("Clear Results");
a = newArray(0, 1, -1, 0.5, 0.0004, 0.0005, 0.00049999, 1e-7, 123456789012, 999999999999, 1234567890123, 1e13, 2147483647,
	2147483648, -2147483648, -2147483649, 2.5, 3.5, 0.125, 0.375, 1.0005, 1.0015, -0.0005, NaN, 1/0, -1/0,
	1.5e300, 12345.6789, -98765.4321, 0.001, 0.0009995, 99999.9995);
for (i=0; i<a.length; i++) setResult("A", i, a[i]);
for (i=0; i<a.length; i++) setResult("B", i, i*37-100);
for (i=0; i<a.length; i++) setResult("C", i, i);
setResult("C", 3, 2147483648);
setResult("C", 4, -5000000000);
for (i=0; i<a.length; i++) setResult("S", i, "t"+i);
setResult("S", 1, "with,comma");
setResult("S", 2, "");
setResult("S", 3, "two\nlines");
setResult("S", 4, "tab\there");
setResult("S", 5, "say \"hi\"");
setResult("Label", 0, "first");
setResult("Label", 2, "a,b");
saveAs("Results", d+"/results_numbers.csv");
saveAs("Results", d+"/results_numbers.txt");
run("Set Measurements...", "area mean min scientific redirect=None decimal=5");
saveAs("Results", d+"/results_numbers_sci5.csv");
run("Set Measurements...", "area mean min redirect=None decimal=0");
saveAs("Results", d+"/results_numbers_dec0.csv");

// A measured row, then rows that leave cells unfilled.
run("Set Measurements...", "area mean min display scientific redirect=None decimal=4");
run("Clear Results");
open(d+"/gray8.tif");
run("Measure");
setResult("Foo", 1, 0.5);
setResult("Label", 1, "a,b");
setResult("Foo", 2, 1234567890123);
saveAs("Results", d+"/results_rows.csv");
close();

// "NaN empty cells" is read when the next command starts, so it takes effect after Clear Results.
run("Set Measurements...", "area mean min nan redirect=None decimal=3");
run("Clear Results");
setResult("A", 0, 1.5);
setResult("A", 1, 2);
setResult("A", 2, 7);
setResult("B", 2, 5);
setResult("S", 1, "text");
saveAs("Results", d+"/results_nan_cells.csv");
run("Set Measurements...", "area mean min redirect=None decimal=3");
run("Clear Results");
print("GEN_OK");

// Measures one selection on one image under the given settings and saves the table.
function one(file, image, position, roi, options) {
	run("Set Measurements...", options);
	run("Clear Results");
	open(d+"/"+image);
	if (position!="-") {
		p = split(position, " ");
		Stack.setPosition(parseInt(p[0]), parseInt(p[1]), parseInt(p[2]));
	}
	if (image=="gray8_calibrated.tif") run("Calibrate...", "function=None");
	a = split(roi, " ");
	if (a[0]=="rect") makeRectangle(parseInt(a[1]), parseInt(a[2]), parseInt(a[3]), parseInt(a[4]));
	if (a[0]=="line") makeLine(parseFloat(a[1]), parseFloat(a[2]), parseFloat(a[3]), parseFloat(a[4]));
	if (a[0]=="point") makePoint(parseInt(a[1]), parseInt(a[2]));
	run("Measure");
	if (nResults!=1) exit("unexpected nResults "+nResults);
	saveAs("Results", d+"/results_"+file);
	close();
	index = index + "results_"+file+"\t"+image+"\t"+position+"\t"+roi+"\t"+options+"\n";
}
