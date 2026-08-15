// Extrusion verification coupon — end-to-end check at profile speeds.
// Features and what they read (see docs/verification/README.md):
//   rib   1.36 mm = exactly 2 beads at w = 0.68  -> flow + two-bead bond
//   plate 60.00 x 30.00                           -> flow + dimensional
//   boss  D10.00 / hole D8.00                     -> informational
// Every Z lands on the 0.2 + n*0.25 layer grid so nominal = printed:
//   plate top  5.95  (n = 23)
//   boss  top  9.95  (n = 39)
//   rib   top 15.95  (n = 63)
// Render: openscad -o extrusion_coupon.stl extrusion_coupon.scad
$fn = 128;

plate_h = 5.95;

difference() {
    union() {
        // plate, centered on origin
        translate([-30, -15, 0]) cube([60, 30, plate_h]);
        // boss D10 x 4
        translate([-19, 0, plate_h]) cylinder(h = 4.0, d = 10.0);
        // rib: 1.36 wide, 20 long, 10 tall
        translate([18 - 1.36/2, -10, plate_h]) cube([1.36, 20, 10.0]);
    }
    // through hole D8
    translate([0, 0, -1]) cylinder(h = plate_h + 2, d = 8.0);
}
