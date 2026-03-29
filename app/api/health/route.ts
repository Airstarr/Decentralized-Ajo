import { NextResponse } from 'next/server';
import { prisma } from '@/lib/prisma';
import { createChildLogger } from '@/lib/logger';

const logger = createChildLogger({ service: 'api', route: '/api/health' });

export async function GET() {
  const start = Date.now();

  try {
    await prisma.$queryRaw`SELECT 1`;

    return NextResponse.json({
      status: 'healthy',
      timestamp: new Date().toISOString(),
      uptime: process.uptime(),
      responseTime: `${Date.now() - start}ms`,
      services: {
        database: 'up',
      },
    });
  } catch (error) {
    logger.error('Health check failed', { err: error });

    return NextResponse.json(
      {
        status: 'unhealthy',
        timestamp: new Date().toISOString(),
        services: {
          database: 'down',
        },
      },
      { status: 503 }
    );
  }
}
