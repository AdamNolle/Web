import { z } from 'zod';

export const SourceSchema = z.object({
  id: z.string(),
  kind: z.enum(['demo', 'rss', 'bluesky', 'mastodon']),
  label: z.string(),
  detail: z.string(),
  status: z.enum(['healthy', 'rate_limited', 'auth_required', 'transient', 'paused']),
  healthDetail: z.string(),
  commentsStatus: z.enum(['unavailable', 'complete', 'partial']),
  commentsTruncated: z.boolean(),
  syncFinality: z.enum(['complete', 'partial']),
  lastSync: z.string(),
  nextSync: z.string().nullable(),
  itemCount: z.number().int().nonnegative(),
});

export const EvidenceSchema = z.object({
  source: z.string(),
  author: z.string(),
  publishedAt: z.string(),
  timestampKind: z.enum(['published', 'updated', 'fetched']),
  excerpt: z.string(),
  canonicalUrl: z.string().url().nullable(),
});

export const DigestItemSchema = z.object({
  id: z.string(),
  sourceId: z.string(),
  source: z.string(),
  author: z.string(),
  title: z.string(),
  summary: z.string(),
  commentOverview: z.string(),
  summaryMethod: z.enum(['fixture', 'extractive', 'model']),
  summaryProvider: z.string(),
  summaryUncertainty: z.string(),
  publishedAt: z.string(),
  publishedTimeKind: z.enum(['published', 'updated', 'fetched']),
  reason: z.string(),
  topic: z.string(),
  importance: z.number().min(0).max(1),
  evidence: z.array(EvidenceSchema).min(1),
});

export const TrendSchema = z.object({
  id: z.string(),
  label: z.string(),
  summary: z.string(),
  sourceCount: z.number().int().positive(),
  confidence: z.enum(['emerging', 'supported']),
  method: z.enum(['fixture', 'lexical', 'embedding']),
  evidenceIds: z.array(z.string()),
});

export const ActivitySchema = z.object({
  id: z.string(),
  kind: z.string(),
  status: z.enum(['queued', 'complete', 'partial', 'running', 'scheduled', 'failed']),
  message: z.string(),
  occurredAt: z.string(),
});

export const SettingsSchema = z.object({
  scheduleEnabled: z.boolean(),
  scheduleHour: z.number().int().min(0).max(23),
  quietHoursStart: z.number().int().min(0).max(23),
  quietHoursEnd: z.number().int().min(0).max(23),
  retentionDays: z.number().int().min(1).max(365),
  remoteMedia: z.boolean(),
  localOnly: z.literal(true),
  feedbackCount: z.number().int().nonnegative(),
  selectedModel: z
    .string()
    .max(200)
    .regex(/^$|^[A-Za-z0-9._:/-]+$/),
  rankingPaused: z.boolean(),
});

export const ModelStateSchema = z.enum([
  'checking',
  'unknown',
  'runtime_unavailable',
  'model_missing',
  'incompatible',
  'detected_unverified',
  'ready',
  'degraded',
]);

export const CapabilityStatusSchema = z.object({
  state: z.enum(['unknown', 'available', 'unavailable', 'degraded']),
  detail: z.string(),
});

export const ModelStatusSchema = z.object({
  provider: z.string(),
  state: ModelStateSchema,
  model: z.string().nullable(),
  digest: z.string().nullable(),
  sizeBytes: z.number().int().nonnegative().nullable(),
  parameterSize: z.string().nullable(),
  quantization: z.string().nullable(),
  runtimeVersion: z.string().nullable(),
  structuredOutput: z.boolean(),
  endpoint: z.string(),
  fallbackAvailable: z.boolean(),
  detail: z.string(),
});

export const HostCapabilitiesSchema = z.object({
  os: z.string(),
  arch: z.string(),
  totalMemoryGb: z.number().nonnegative(),
  availableMemoryGb: z.number().nonnegative(),
  logicalCpuCount: z.number().int().nonnegative(),
  gpu: CapabilityStatusSchema,
  battery: CapabilityStatusSchema,
  meteredNetwork: CapabilityStatusSchema,
  localRuntime: CapabilityStatusSchema,
  recommendedProfile: z.object({
    id: z.enum(['cpu-basic', 'balanced', 'performance']),
    title: z.string(),
    generationModel: z.string(),
    embeddingModel: z.string(),
    contextWindow: z.number().int().positive(),
    maxConcurrentRequests: z.literal(1),
    rationale: z.string(),
    requiresExplicitDownload: z.literal(true),
  }),
});

export const SyncOutcomeSchema = z.object({
  mode: z.enum(['manual_override', 'resident_due']),
  finality: z.enum(['complete', 'partial', 'unknown']),
  changedSources: z.number().int().nonnegative(),
  unchangedSources: z.number().int().nonnegative(),
  failedSources: z.number().int().nonnegative(),
  changedItems: z.number().int().nonnegative(),
  sourceLimitReached: z.boolean(),
});

export const ConnectorDescriptorSchema = z.object({
  kind: z.enum(['rss', 'mastodon', 'bluesky']),
  label: z.string(),
  availability: z.enum(['available', 'validation_required', 'blocked']),
  detail: z.string(),
  unmetPrerequisite: z.string().nullable(),
  readOnly: z.literal(true),
  supportsComments: z.boolean(),
  requiresOauth: z.boolean(),
});

export const DashboardSchema = z.object({
  privacyEpoch: z.number().int().nonnegative(),
  edition: z.object({
    id: z.string(),
    label: z.string(),
    generatedAt: z.string(),
    nextEditionAt: z.string().nullable(),
    summary: z.string(),
  }),
  items: z.array(DigestItemSchema).max(12),
  trends: z.array(TrendSchema).max(5),
  sources: z.array(SourceSchema),
  activity: z.array(ActivitySchema).max(20),
  settings: SettingsSchema,
  model: ModelStatusSchema,
  host: HostCapabilitiesSchema,
  connectors: z.array(ConnectorDescriptorSchema).length(3),
  runner: z.object({
    active: z.boolean(),
    inFlight: z.boolean(),
    lastAttemptAt: z.string().nullable(),
    lastSuccessAt: z.string().nullable(),
    nextScheduledAt: z.string().nullable(),
    lastOutcome: z.enum(['idle', 'running', 'complete', 'partial', 'failed', 'unknown']),
    detail: z.string(),
  }),
});

export type Source = z.infer<typeof SourceSchema>;
export type DigestItem = z.infer<typeof DigestItemSchema>;
export type Trend = z.infer<typeof TrendSchema>;
export type Activity = z.infer<typeof ActivitySchema>;
export type Settings = z.infer<typeof SettingsSchema>;
export type ModelStatus = z.infer<typeof ModelStatusSchema>;
export type Dashboard = z.infer<typeof DashboardSchema>;
export type SyncOutcome = z.infer<typeof SyncOutcomeSchema>;
export type SyncSourcesResult = { dashboard: Dashboard; outcome: SyncOutcome };
export type FeedbackSignal = 'more_like_this' | 'less_like_this' | 'not_relevant' | 'mute_source';

export interface AppError {
  code:
    | 'VALIDATION'
    | 'NOT_FOUND'
    | 'AUTH_REQUIRED'
    | 'RATE_LIMITED'
    | 'MODEL_UNAVAILABLE'
    | 'CONFLICT'
    | 'INTERNAL';
  message: string;
  retryable: boolean;
  correlationId: string;
}
