/**
 * In-memory sliding-window rate limiter.
 * For production with multiple instances, swap the store for a Redis/Upstash backend.
 */

import { Redis } from '@upstash/redis';
import { createChildLogger } from './logger';

const logger = createChildLogger({ service: 'rate-limit' });

/**
 * Shared interface for rate limit stores.
 */
interface RateLimitStore {
  check(key: string, config: RateLimitConfig): Promise<{ retryAfter: number } | null>;
}

/**
 * In-memory sliding-window rate limiter for local development.
 */
class MemoryStore implements RateLimitStore {
  private store = new Map<string, { count: number; resetAt: number }>();

  async check(key: string, config: RateLimitConfig): Promise<{ retryAfter: number } | null> {
    const now = Date.now();
    const entry = this.store.get(key);

    if (!entry || now >= entry.resetAt) {
      this.store.set(key, { count: 1, resetAt: now + config.windowMs });
      return null;
    }

    if (entry.count >= config.limit) {
      return { retryAfter: Math.ceil((entry.resetAt - now) / 1000) };
    }

    entry.count += 1;
    return null;
  }
}

/**
 * Redis-backed rate limiter for production (multi-instance).
 * Uses Upstash Redis for compatibility with serverless environments.
 */
class RedisStore implements RateLimitStore {
  private redis: Redis;

  constructor() {
    this.redis = new Redis({
      url: process.env.UPSTASH_REDIS_REST_URL!,
      token: process.env.UPSTASH_REDIS_REST_TOKEN!,
    });
  }

  async check(key: string, config: RateLimitConfig): Promise<{ retryAfter: number } | null> {
    const now = Date.now();
    const identifier = `rl:${key}`;

    try {
      // Use a pipeline or Lua script for atomic increment and expiry
      // For simplicity with Upstash, we use a basic increment pattern
      const count = await this.redis.incr(identifier);

      if (count === 1) {
        await this.redis.pexpire(identifier, config.windowMs);
      }

      if (count > config.limit) {
        const pttl = await this.redis.pttl(identifier);
        return { retryAfter: Math.max(0, Math.ceil(pttl / 1000)) };
      }

      return null;
    } catch (err) {
      // FAIL-OPEN: If Redis is down, log the error and allow the request.
      // This prioritizes availability over strict rate limiting.
      logger.error('Redis rate limit error, failing open', { err, key });
      return null;
    }
  }
}

// Initialize the appropriate store based on environment variables
const store: RateLimitStore =
  process.env.UPSTASH_REDIS_REST_URL && process.env.UPSTASH_REDIS_REST_TOKEN
    ? new RedisStore()
    : new MemoryStore();

export interface RateLimitConfig {
  /** Max requests allowed within the window */
  limit: number;
  /** Window duration in milliseconds */
  windowMs: number;
}

/** Pre-configured limits for different route groups */
export const RATE_LIMITS = {
  /** Tight limit for auth endpoints to slow brute-force attacks */
  auth: { limit: 10, windowMs: 15 * 60 * 1000 } satisfies RateLimitConfig,
  /** General API limit */
  api: { limit: 100, windowMs: 15 * 60 * 1000 } satisfies RateLimitConfig,
  /** Sensitive operations like joining/creating circles */
  sensitive: { limit: 20, windowMs: 60 * 60 * 1000 } satisfies RateLimitConfig,
} as const;

/**
 * Returns `null` when the request is allowed, or a `{ retryAfter }` object
 * (seconds until reset) when the limit is exceeded.
 *
 * NOTE: This is now asynchronous to support Redis stores.
 */
export async function checkRateLimit(
  key: string,
  config: RateLimitConfig,
): Promise<{ retryAfter: number } | null> {
  return await store.check(key, config);
}

/**
 * Derive a rate-limit key from a NextRequest.
 * Uses the authenticated user ID when available, falls back to IP.
 */
export function getRateLimitKey(prefix: string, identifier: string): string {
  return `${prefix}:${identifier}`;
}
