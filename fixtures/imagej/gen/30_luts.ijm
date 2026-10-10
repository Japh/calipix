// Built-in lookup tables, as ImageJ's LutLoader generates them.
// Outputs:
//   luts.csv   one line per LUT: name, then red,green,blue for each of the 256 entries.
//              "<name> inverted" lines are the result of Image > Lookup Tables > Invert LUT.
d = getArgument();
names = newArray("Fire", "Grays", "Ice", "Spectrum", "3-3-2 RGB", "Red", "Green", "Blue", "Cyan", "Magenta", "Yellow", "Red/Green");
newImage("lut", "8-bit ramp", 256, 1, 1);
s = "";
for (n=0; n<names.length; n++) {
	run(names[n]);
	s = s + lutLine(names[n]);
}
run("Fire"); run("Invert LUT");
s = s + lutLine("Fire inverted");
run("Grays"); run("Invert LUT");
s = s + lutLine("Grays inverted");
close();
File.saveString(s, d+"/luts.csv");
print("GEN_OK");

function lutLine(name) {
	getLut(r, g, b);
	line = name;
	for (i=0; i<256; i++) line = line + "," + r[i] + "," + g[i] + "," + b[i];
	return line + "\n";
}
