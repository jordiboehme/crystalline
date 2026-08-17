// WCAG 2.x contrast ratio of two sRGB hex colors.
// Usage: node fluid/scripts/contrast.mjs '#64748b' '#ffffff'
function channel(value) {
  const c = value / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}
function luminance(hex) {
  const n = hex.replace("#", "");
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(n.slice(i, i + 2), 16));
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}
const [a, b] = process.argv.slice(2);
if (!a || !b) {
  console.error("usage: contrast.mjs '#rrggbb' '#rrggbb'");
  process.exit(2);
}
const [la, lb] = [luminance(a), luminance(b)];
console.log(((Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)).toFixed(2));
