import { describe, expect, it } from 'vitest';
import { demoDashboard } from './demoData';
import { createDemoTransport, parseDashboard } from './transport';

describe('transport contracts', () => {
  it('accepts backend-shaped partial activity at the native Zod boundary', () => {
    const value = structuredClone(demoDashboard);
    const first = value.activity[0];
    if (!first) throw new Error('demo activity fixture is required');
    value.activity[0] = { ...first, status: 'partial', message: 'Partial page stored' };
    const parsed = parseDashboard(value);
    expect(parsed.activity[0]?.status).toBe('partial');
  });

  it('makes identical feedback retries idempotent and rejects changed payloads', async () => {
    const transport = createDemoTransport();
    const first = await transport.recordFeedback('request-a', 'post-local-ai', 'more_like_this');
    const replay = await transport.recordFeedback('request-a', 'post-local-ai', 'more_like_this');
    expect(first.settings.feedbackCount).toBe(1);
    expect(replay.settings.feedbackCount).toBe(1);
    await expect(
      transport.recordFeedback('request-a', 'post-local-ai', 'not_relevant'),
    ).rejects.toThrow(/different feedback/);
  });

  it('keeps completed receipt tombstones across reset and makes undo after reset a no-op', async () => {
    const transport = createDemoTransport();
    await transport.recordFeedback('request-a', 'post-local-ai', 'not_relevant');
    await transport.resetLearning('reset-a');
    const delayed = await transport.recordFeedback('request-a', 'post-local-ai', 'not_relevant');
    expect(delayed.items.some((item) => item.id === 'post-local-ai')).toBe(true);
    const undone = await transport.undoFeedback('request-a');
    expect(undone.items.some((item) => item.id === 'post-local-ai')).toBe(true);
    expect(undone.settings.feedbackCount).toBe(0);
  });
});
