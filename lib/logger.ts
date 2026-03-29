import fs from 'node:fs';
import path from 'node:path';
import winston from 'winston';
import DailyRotateFile from 'winston-daily-rotate-file';

const { combine, timestamp, errors, json, colorize, printf } = winston.format;

const isProduction = process.env.NODE_ENV === 'production';
const logsDir = path.resolve(process.cwd(), 'logs');
const SENSITIVE_KEYS = new Set([
  'password',
  'token',
  'jwt',
  'authorization',
  'cookie',
  'secret',
  'apiKey',
  'privateKey',
]);

fs.mkdirSync(logsDir, { recursive: true });

function sanitizeLogValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => sanitizeLogValue(item));
  }

  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).map(([key, nestedValue]) => [
        key,
        SENSITIVE_KEYS.has(key) ? '[REDACTED]' : sanitizeLogValue(nestedValue),
      ]),
    );
  }

  return value;
}

const redactSecrets = winston.format((info) => sanitizeLogValue(info) as winston.Logform.TransformableInfo);

const devFormat = combine(
  colorize(),
  timestamp({ format: 'YYYY-MM-DD HH:mm:ss' }),
  errors({ stack: true }),
  redactSecrets(),
  printf(({ timestamp: ts, level, message, stack, service, route, module, ...meta }) => {
    const context = [service, route, module].filter(Boolean).join(' ');
    const metaPayload = stack ? { ...meta, stack } : meta;
    const metaStr = Object.keys(metaPayload).length > 0 ? ` ${JSON.stringify(metaPayload)}` : '';
    return `${ts} [${level}]${context ? ` ${context}` : ''}: ${message}${metaStr}`;
  }),
);

const prodFormat = combine(
  timestamp(),
  errors({ stack: true }),
  redactSecrets(),
  json(),
);

const transports: winston.transport[] = [
  new DailyRotateFile({
    filename: path.join(logsDir, 'error-%DATE%.log'),
    datePattern: 'YYYY-MM-DD',
    level: 'error',
    maxSize: '20m',
    maxFiles: '14d',
    format: prodFormat,
  }),
  new DailyRotateFile({
    filename: path.join(logsDir, 'combined-%DATE%.log'),
    datePattern: 'YYYY-MM-DD',
    maxSize: '20m',
    maxFiles: '14d',
    format: prodFormat,
  }),
];

if (!isProduction) {
  transports.push(
    new winston.transports.Console({
      format: devFormat,
    }),
  );
}

export const logger = winston.createLogger({
  level: process.env.LOG_LEVEL || (isProduction ? 'info' : 'debug'),
  defaultMeta: {
    env: process.env.NODE_ENV || 'development',
    service: 'backend',
  },
  transports,
  exitOnError: false,
});

export function createChildLogger(meta: Record<string, unknown>) {
  return logger.child(meta);
}

export default logger;
