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
      // light theme: input/control borders against their surfaces
      ['#85858d', '#ffffff'],
      ['#85858d', '#f4f4f5'],
      // dark theme: input/control borders against their surfaces
      ['#8d8d95', '#18181b'],
      // brand mark: white disc on the purple square, and the square on each sidebar
      ['#ffffff', '#5b21b6'],
      ['#5b21b6', '#f4f4f5'],
      ['#ffffff', '#8b5cf6'],
      ['#8b5cf6', '#18181b'],
    ] as const;
    for (const [foreground, surface] of pairs) {
      expect(contrast(foreground, surface)).toBeGreaterThanOrEqual(3);
    }
    expect(css).toContain('--input-bg: #ffffff');
    expect(css).toContain('--input-border: #85858d');
    expect(css).toContain('--input-bg: #1f1f23');
    expect(css).toContain('--input-border: #8d8d95');
    expect(css).toContain('--control-border: #85858d');
    expect(css).toContain('--control-border: #8d8d95');
    expect(css).toContain('--brand-fill: #5b21b6');
    expect(css).toContain('--brand-border: #000000');
    expect(css).toContain('--brand-disc: #ffffff');
    expect(css).toContain('--brand-fill: #8b5cf6');
    expect(css).toMatch(
      /\.brand-mark \{[\s\S]*background: var\(--brand-fill\)[\s\S]*\.brand-mark::after \{[\s\S]*background: var\(--brand-disc\)/,
    );
    // body text pairs (4.5:1 — small text, both themes)
    expect(contrast('#3f3f46', '#ffffff')).toBeGreaterThanOrEqual(4.5);
    expect(contrast('#d4d4d8', '#18181b')).toBeGreaterThanOrEqual(4.5);
    expect(contrast('#71717a', '#ffffff')).toBeGreaterThanOrEqual(4.5);
    expect(contrast('#a1a1aa', '#18181b')).toBeGreaterThanOrEqual(4.5);
    // accent text pairs (used for eyebrow/why/topic-badge/links)
    expect(contrast('#5b21b6', '#ffffff')).toBeGreaterThanOrEqual(4.5);
    expect(contrast('#a78bfa', '#18181b')).toBeGreaterThanOrEqual(4.5);
    expect(contrast('#4c1d95', '#ede9fe')).toBeGreaterThanOrEqual(4.5);
    expect(contrast('#ddd6fe', '#3b2f63')).toBeGreaterThanOrEqual(4.5);
    // danger text pairs
    expect(contrast('#b91c1c', '#ffffff')).toBeGreaterThanOrEqual(4.5);
    expect(contrast('#fca5a5', '#18181b')).toBeGreaterThanOrEqual(4.5);
    expect(css).toMatch(
      /\.secondary[\s\S]*border: 1px solid var\(--glass-border\)[\s\S]*background: var\(--glass-bg\)/,
    );
    expect(css).toMatch(/\.add-source input[\s\S]*background: var\(--input-bg\)/);
    expect(css).toMatch(/\.quiet-hours[\s\S]*border: 1px solid var\(--input-border\)/);
    expect(css).toMatch(
      /\.save-bar button:focus-visible,[\s\S]*outline-color: var\(--focus-on-dark\)/,
    );
  });

  it('no longer relies on the retired earthy/serif tokens', () => {
    expect(css).not.toMatch(/--ochre/);
    expect(css).not.toMatch(/--green\b/);
    expect(css).not.toMatch(/Newsreader/);
    expect(css).not.toMatch(/#f3f1ea|#fbfaf6|#eeece4|#f0eee7|#ae7441/i);
  });
});
