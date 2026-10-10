// LUT and calibration variants of the base planes (depends on 01_planes.ijm).
// Outputs:
//   gray8_invlut.tif        8-bit with inverting LUT (pixel data unchanged, PhotometricInterpretation=WhiteIsZero)
//   gray8_fire.tif          8-bit with the "Fire" colour LUT (palette TIFF, ColorMap tag)
//   gray16_signed_src.raw   big-endian raw dump of gray16 (reinterpreted below as signed)
//   gray16_signed.tif       signed 16-bit: ImageJ stores value+32768 plus calibration function cf=... c0=-32768 c1=1
//   gray8_calibrated.tif    8-bit with spatial calibration in mm + origin, and a linear density calibration
//   gray8_roi_overlay.tif   8-bit with an active oval ROI and a 2-element overlay ('roi ' and 'over' metadata blocks)
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);

open(d+"/gray8.tif");
run("Invert LUT");
print("inverting LUT: " + is("Inverting LUT") + " pixel(1,0)=" + getPixel(1,0));
saveAs("Tiff", d+"/gray8_invlut.tif");
close();

open(d+"/gray8.tif");
run("Fire");
saveAs("Tiff", d+"/gray8_fire.tif");
close();

open(d+"/gray16.tif");
saveAs("Raw Data", d+"/gray16_signed_src.raw");
close();
run("Raw...", "open=["+d+"/gray16_signed_src.raw] image=[16-bit Signed] width=64 height=48 offset=0 number=1 gap=0");
print("signed16: raw(40,47)=" + getPixel(40,47) + " calibrated=" + calibrate(getPixel(40,47)));
saveAs("Tiff", d+"/gray16_signed.tif");
close();

open(d+"/gray8.tif");
run("Properties...", "channels=1 slices=1 frames=1 pixel_width=0.5 pixel_height=0.25 voxel_depth=2 unit=mm origin=3,4");
run("Calibrate...", "function=[Straight Line] unit=[OD] text1=[0 255] text2=[0.5 3.05]");
getPixelSize(u, pw, ph);
print("calibrated: " + pw + " x " + ph + " " + u + " value(0)=" + calibrate(0) + " value(255)=" + calibrate(255));
saveAs("Tiff", d+"/gray8_calibrated.tif");
close();

open(d+"/gray8.tif");
makeRectangle(4, 4, 10, 10); Overlay.addSelection("red");
makeLine(2, 40, 60, 44); Overlay.addSelection("#00ff00", 2);
makeOval(20, 10, 15, 20);
print("overlay size " + Overlay.size + " selectionType " + selectionType());
saveAs("Tiff", d+"/gray8_roi_overlay.tif");
close();
print("GEN_OK");
