import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { describe, expect, it } from 'vitest';

const sha256 = (value: string) => createHash('sha256').update(value).digest('hex');

describe('artifact checksum generator', () => {
  it('writes a sorted manifest and accepts pnpm’s argument separator', () => {
    const directory = mkdtempSync(join(tmpdir(), 'web-checksums-'));
    try {
      const alpha = join(directory, 'alpha.bin');
      const beta = join(directory, 'beta.bin');
      const manifest = join(directory, 'SHA256SUMS.txt');
      writeFileSync(alpha, 'alpha', 'utf8');
      writeFileSync(beta, 'beta', 'utf8');

      const result = spawnSync(
        process.execPath,
        ['scripts/checksums.mjs', '--', '--output', manifest, beta, alpha],
        { cwd: process.cwd(), encoding: 'utf8' },
      );

      expect(result.status).toBe(0);
      expect(result.stderr).toBe('');
      expect(readFileSync(manifest, 'utf8')).toBe(
        `${sha256('alpha')}  ${relative(process.cwd(), alpha).replaceAll('\\', '/')}\n${sha256('beta')}  ${relative(process.cwd(), beta).replaceAll('\\', '/')}\n`,
      );
    } finally {
      rmSync(directory, { force: true, recursive: true });
    }
  });
});
