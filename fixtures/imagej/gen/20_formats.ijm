// JPEG, GIF and BMP files written by ImageJ, and what ImageJ reads back from the lossy or
// converted ones. (Depends on 01: the driver runs gen/*.ijm in lexical order.)
// The pixel truth for gray8.* and rgb.* is gray8.png / rgb.png from 01_planes.ijm.
//   fmt_anim pattern: v = (x*7 + y*3 + s*40) mod 256 for slice s = 1..5 of a 32x24 stack
// Outputs:
//   fmt_gray8.jpg fmt_rgb.jpg            saved at quality 85
//   fmt_gray8_jpg.png fmt_rgb_jpg.png    those JPEGs as ImageJ decodes them
//   fmt_gray8.gif fmt_gray8.bmp fmt_rgb.bmp
//   fmt_fire.gif fmt_fire.bmp            gray8 with the Fire lookup table
//   fmt_fire_lut.csv                     that table, one "r,g,b" line per entry
//   fmt_anim.gif                         8-bit stack saved as an animated GIF
//   fmt_anim_fire.gif                    the same stack with the Fire table
//   fmt_anim_fire_<s>.png                its frames as ImageJ reads them back (RGB)
//   fmt_reopened.csv                     name,bitDepth,slices,channels,frames of each file reopened
d = getArgument();
run("Input/Output...", "jpeg=85 gif=-1 file=.csv save_column save_row");
setOption("BlackBackground", true);
info = "";

function describe(name) {
	getDimensions(w, h, channels, slices, frames);
	return name+","+bitDepth()+","+slices+","+channels+","+frames+"\n";
}

open(d+"/gray8.tif");
saveAs("Jpeg", d+"/fmt_gray8.jpg");
saveAs("Gif", d+"/fmt_gray8.gif");
saveAs("BMP", d+"/fmt_gray8.bmp");
run("Fire");
saveAs("Gif", d+"/fmt_fire.gif");
saveAs("BMP", d+"/fmt_fire.bmp");
getLut(r, g, b);
s = "";
for (i=0; i<r.length; i++) s = s+r[i]+","+g[i]+","+b[i]+"\n";
File.saveString(s, d+"/fmt_fire_lut.csv");
close();

open(d+"/rgb.tif");
saveAs("Jpeg", d+"/fmt_rgb.jpg");
saveAs("BMP", d+"/fmt_rgb.bmp");
close();

newImage("fmt_anim", "8-bit black", 32, 24, 5);
for (s=1; s<=5; s++) {
	setSlice(s);
	for (y=0; y<24; y++) for (x=0; x<32; x++) setPixel(x, y, (x*7 + y*3 + s*40) % 256);
}
setSlice(1);
saveAs("Gif", d+"/fmt_anim.gif");
run("Fire");
saveAs("Gif", d+"/fmt_anim_fire.gif");
close();

names = newArray("fmt_gray8.jpg", "fmt_rgb.jpg", "fmt_gray8.gif", "fmt_fire.gif", "fmt_gray8.bmp",
	"fmt_fire.bmp", "fmt_rgb.bmp", "fmt_anim.gif", "fmt_anim_fire.gif");
for (i=0; i<names.length; i++) {
	open(d+"/"+names[i]);
	info = info + describe(names[i]);
	if (names[i]=="fmt_gray8.jpg") saveAs("PNG", d+"/fmt_gray8_jpg.png");
	if (names[i]=="fmt_rgb.jpg") saveAs("PNG", d+"/fmt_rgb_jpg.png");
	if (names[i]=="fmt_anim_fire.gif") {
		n = nSlices;
		for (s=1; s<=n; s++) {
			setSlice(s);
			run("Duplicate...", "title=frame");
			saveAs("PNG", d+"/fmt_anim_fire_"+s+".png");
			close();
		}
	}
	close();
}
File.saveString(info, d+"/fmt_reopened.csv");
print("GEN_OK");
