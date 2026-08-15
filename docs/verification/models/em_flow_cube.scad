// EM flow cube — single-wall extrusion-multiplier test.
// Solid on purpose: the slicer preset (slicer/em-flowcube-presets.ini)
// hollows it to one external perimeter.
// Height 29.95 sits on the 0.2 + n*0.25 layer grid (n = 119), so the
// modeled height is the printed height exactly.
// Render: openscad -o em_flow_cube.stl em_flow_cube.scad
cube([40, 40, 29.95], center = false);
