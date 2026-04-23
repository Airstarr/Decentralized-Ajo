import { NextRequest, NextResponse } from 'next/server';
import { Prisma } from '@prisma/client';
import { prisma } from '@/lib/prisma';
import { verifyToken, extractToken } from '@/lib/auth';
import { validateBody, applyRateLimit } from '@/lib/api-helpers';
import { UpdateProfileSchema } from '@/lib/validations/user';
import { RATE_LIMITS } from '@/lib/rate-limit';
import { createChildLogger } from '@/lib/logger';

const logger = createChildLogger({ service: 'api', route: '/api/profile' });

const USER_SELECT = {
  id: true,
  email: true,
  username: true,
  firstName: true,
  lastName: true,
  notificationEmail: true,
  bio: true,
  phoneNumber: true,
  profilePicture: true,
  walletAddress: true,
  createdAt: true,
};

export async function GET(request: NextRequest) {
  const token = extractToken(request.headers.get('authorization'));
  if (!token) return NextResponse.json({ error: 'Unauthorized' }, { status: 401 });

  const payload = verifyToken(token);
  if (!payload) return NextResponse.json({ error: 'Invalid or expired token' }, { status: 401 });

  const rateLimited = await applyRateLimit(request, RATE_LIMITS.api, 'profile:get', payload.userId);
  if (rateLimited) return rateLimited;

  try {
    const user = await prisma.user.findUnique({ where: { id: payload.userId }, select: USER_SELECT });
    if (!user) return NextResponse.json({ error: 'User not found' }, { status: 404 });
    return NextResponse.json({ success: true, user });
  } catch (error) {
    logger.error('Get profile error', { err: error, userId: payload.userId });
    return NextResponse.json({ error: 'Internal server error' }, { status: 500 });
  }
}

export async function PUT(request: NextRequest) {
  const token = extractToken(request.headers.get('authorization'));
  let payload = token ? verifyToken(token) : null;

  const userAddress = request.headers.get('x-wallet-address');
  
  if (!payload && !userAddress) {
    return NextResponse.json({ error: 'Unauthorized - missing token or wallet address' }, { status: 401 });
  }

  let userIdOrAddress = payload ? payload.userId : userAddress;
  const rateKey = payload ? payload.userId : `wallet:${userAddress}`;
  const rateLimited = applyRateLimit(request, RATE_LIMITS.api, 'profile-update', rateKey);
  if (rateLimited) return rateLimited;

  const body = await request.json();
  const { username, email } = body;
  if (username === undefined || email === undefined) {
    return NextResponse.json({ error: 'Missing username or email' }, { status: 400 });
  }

  // Validate using subset of schema
  const partialData = { username, email };
  const validation = UpdateProfileSchema.partial().safeParse(partialData);
  if (!validation.success) {
    return NextResponse.json({ error: validation.error.errors[0].message }, { status: 400 });
  }

  const validatedData = {
    username: username.trim() === '' ? null : username.trim(),
    email: email.trim().toLowerCase(),
  };

  try {
    let user;
    if (payload) {
      // Existing token flow
      user = await prisma.user.update({
        where: { id: payload.userId },
        data: validatedData,
        select: USER_SELECT,
      });
    } else {
      // Wallet upsert flow
      user = await prisma.user.upsert({
        where: { address: userAddress! },
        update: validatedData,
        create: { 
          address: userAddress!,
          username: validatedData.username!,
          email: validatedData.email!,
        },
        select: { id: true, address: true, username: true, email: true },
      });
    }

    return NextResponse.json({ success: true, user });
  } catch (err: any) {
    if (err.code === 'P2002') {
      return NextResponse.json({ error: 'Username or email already taken' }, { status: 409 });
    }
    logger.error('Update profile error', { err, userId: userIdOrAddress });
    return NextResponse.json({ error: 'Database mutation failed' }, { status: 500 });
  }
}
