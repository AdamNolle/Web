import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync(`${process.cwd()}/src/styles.css`, 'utf8');
const rgb = (hex: string) =>
  [1, 3, 5].map((offset) => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255);
const luminance = (hex: string) => {
  const channels = rgb(hex).map((value) =>
    value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4,
  );
  return 0.2126 * channels[0]! + 0.7152 * channels[1]! + 0.0722 * channels[2]!;
};
const contrast = (first: string, second: string) => {
  const [lighter, darker] = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (lighter! + 0.05) / (darker! + 0.05);
};

describe('declared non-text contrast tokens', () => {
  it('keeps actual light and dark input/focus surface pairs above 3:1', () => {
    const pairs = [
      ['#747970', '#fffef9'],
      ['#8d958c', '#2a2d29'],
      ['#1f6043', '#eeece4'],
      ['#a8efbd', '#252824'],
      ['#ffffff', '#252824'],
      ['#a8efbd', '#252824'],
      ['#747970', '#fbfaf6'],
      ['#8d958c', '#222522'],
      // brand mark: white disc on the blue square, and the square on each sidebar
      ['#ffffff', '#1d68f0'],
      ['#1d68f0', '#eeece4'],
      ['#1d68f0', '#1c1f1c'],
    ] as const;
    for (const [foreground, surface] of pairs) {
      expect(contrast(foreground, surface)).toBeGreaterThanOrEqual(3);
    }
    expect(css).toContain('--input-bg: #fffef9');
    expect(css).toContain('--input-border: #747970');
    expect(css).toContain('--input-bg: #2a2d29');
    expect(css).toContain('--input-border: #8d958c');
    expect(css).toContain('--control-border: #747970');
    expect(css).toContain('--control-border: #8d958c');
    expect(css).toContain('--brand-blue: #1d68f0');
    expect(css).toContain('--brand-disc: #ffffff');
    expect(css).toMatch(
      /\.brand-mark \{[\s\S]*background: var\(--brand-blue\)[\s\S]*\.brand-mark::after \{[\s\S]*background: var\(--brand-disc\)/,
    );
    expect(contrast('#404740', '#fbfaf6')).toBeGreaterThanOrEqual(4.5);
    expect(contrast('#e0e3de', '#222522')).toBeGreaterThanOrEqual(4.5);
    expect(css).toMatch(
      /\.secondary[\s\S]*border: 1px solid var\(--control-border\)[\s\S]*color: var\(--control-text\)/,
    );
    expect(css).toMatch(/\.add-source input[\s\S]*background: var\(--input-bg\)/);
    expect(css).toMatch(/\.quiet-hours[\s\S]*border: 1px solid var\(--input-border\)/);
    expect(css).toMatch(
      /\.save-bar button:focus-visible,[\s\S]*outline-color: var\(--focus-on-dark\)/,
    );
  });
});
