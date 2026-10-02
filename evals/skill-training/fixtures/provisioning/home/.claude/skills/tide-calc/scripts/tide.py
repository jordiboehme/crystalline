"""Height of tide by the rule of twelfths."""
import sys

TWELFTHS = [0, 1, 3, 6, 9, 11, 12]

rng, hours = float(sys.argv[1]), int(sys.argv[2])
print(round(rng * TWELFTHS[max(0, min(hours, 6))] / 12, 2))
