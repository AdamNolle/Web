import { createHash } from 'node:crypto';
import { createReadStream, statSync, writeFileSync } from 'node:fs';
import { relative, resolve, sep } from 'node:path';

const args = process.argv.slice(2);
if (args[0] === '--') args.shift();
const outputIndex = args.indexOf('--output');
let output;
if (outputIndex !== -1) {
  output = args[outputIndex + 1];
  if (!output) throw new Error('Expected a file path after --output.');
  args.splice(outputIndex, 2);
}
if (args.length === 0) {
  throw new Error('Usage: node scripts/checksums.mjs [--output SHA256SUMS.txt] <artifact>...');
}

const digest = (file) =>
  new Promise((resolveDigest, reject) => {
    const hash = createHash('sha256');
    const stream = createReadStream(file);
    stream.on('error', reject);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.on('end', () => resolveDigest(hash.digest('hex')));
  });

const cwd = process.cwd();
const artifacts = await Promise.all(
  args.map(async (input) => {
    const absolute = resolve(cwd, input);
    if (!statSync(absolute).isFile()) throw new Error(`Artifact is not a file: ${input}`);
    const name = relative(cwd, absolute).split(sep).join('/');
    return { digest: await digest(absolute), name };
  }),
);
const manifest = artifacts
  .sort((left, right) => left.name.localeCompare(right.name))
  .map((artifact) => `${artifact.digest}  ${artifact.name}`)
  .join('\n')
  .concat('\n');

if (output) writeFileSync(resolve(cwd, output), manifest, { encoding: 'utf8' });
else process.stdout.write(manifest);
