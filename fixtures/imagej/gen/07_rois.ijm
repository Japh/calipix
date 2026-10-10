// ROI fixtures.
// Outputs: roi_oval.roi (single ImageJ .roi), roiset.zip (ROI Manager set: rectangle, oval, polygon, line, point)
// NOTE: roiset.zip embeds zip-entry timestamps, so it is NOT byte-reproducible between runs
// (the driver records it as volatile; compare the contained .roi payloads instead).
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
newImage("roi-host", "8-bit black", 64, 48, 1);
makeOval(20, 10, 15, 20);
saveAs("Selection", d+"/roi_oval.roi");
makeRectangle(4, 4, 10, 10); roiManager("Add");
makeOval(20, 10, 15, 20); roiManager("Add");
makePolygon(40, 5, 60, 10, 50, 40); roiManager("Add");
makeLine(2, 40, 60, 44); roiManager("Add");
makePoint(33, 17); roiManager("Add");
print("roi count=" + roiManager("count"));
roiManager("Save", d+"/roiset.zip");
print("GEN_OK");
