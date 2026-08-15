# Prusament PETG Carbon Fiber on a DiamondBack 0.8, CORE One L.
# The material this project was originally built around.
name = Prusament PETG CF @ 0.8 DiamondBack

note = Prusa publishes filament_max_volumetric_speed = 22 mm3/s for this
note = material at 0.8 (see reference/ref.gcode header).
note = MEASURED 2026-08-11: visual wall failure appeared at band 5-6, i.e.
note = ~16-18 mm3/s -- BELOW Prusa's published 22. Filament was not dried
note = for that run, so treat the number as provisional.

reference = reference/ref-petg.gcode  # MISSING: ref.gcode turned out to be PC

temps = 255, 265, 275, 285
flows = 8, 10, 12, 14, 16, 18, 20, 22, 24
bed = 85
fan = 128

nozzle = 0.8
layer_h = 0.4
width = 0.9
first_layer_h = 0.2
first_layer_w = 1.0
first_layer_flow = 8
em = 1.03
pa = 0.018
retract = 0.8
brim = 3
diameter = 50
revs = 4
