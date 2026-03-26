// GET - List circles with pagination, filtering, and sorting
export async function GET(request: NextRequest) {
  const token = extractToken(request.headers.get('authorization'));
  if (!token) {
    return NextResponse.json({ error: 'Unauthorized' }, { status: 401 });
  }

  const payload = verifyToken(token);
  if (!payload) {
    return NextResponse.json({ error: 'Invalid or expired token' }, { status: 401 });
  }

  const rateLimited = applyRateLimit(request, RATE_LIMITS.api, 'circles:list', payload.userId);
  if (rateLimited) return rateLimited;

  try {
    // Parse and validate query params
    const { searchParams } = request.nextUrl;
    const page = Math.max(1, parseInt(searchParams.get('page') ?? '1', 10) || 1);
    const limit = Math.min(100, Math.max(1, parseInt(searchParams.get('limit') ?? '10', 10) || 10));
    const statusParam = searchParams.get('status')?.toUpperCase();
    const durationParam = searchParams.get('duration'); // Weekly, Monthly, Quarterly
    const sortBy = searchParams.get('sortBy') || 'newest'; // newest, size_desc, size_asc, name_asc, name_desc
    const searchQuery = searchParams.get('search') || '';

    // Validate status value if provided
    if (statusParam && !(statusParam in CircleStatus)) {
      return NextResponse.json(
        { error: `Invalid status. Must be one of: ${Object.values(CircleStatus).join(', ')}` },
        { status: 400 }
      );
    }

    const skip = (page - 1) * limit;
    const search = searchParams.get('search')?.trim();

    // Build where clause
    let where: any = {
      OR: [
        { organizerId: payload.userId },
        { members: { some: { userId: payload.userId } } },
      ],
      ...(statusParam ? { status: statusParam as CircleStatus } : {}),
      ...(search
        ? {
            OR: [
              { name: { contains: search, mode: 'insensitive' as const } },
              { description: { contains: search, mode: 'insensitive' as const } },
            ],
          }
        : {}),
    };
    
    // Add status filter
    if (statusParam) {
      where.status = statusParam as CircleStatus;
    }
    
    // Add duration filter based on contributionFrequencyDays
    if (durationParam) {
      if (durationParam === 'Weekly') {
        where.contributionFrequencyDays = 7;
      } else if (durationParam === 'Monthly') {
        where.contributionFrequencyDays = 30;
      } else if (durationParam === 'Quarterly') {
        where.contributionFrequencyDays = 90;
      }
    }

    // Add search filter
    if (searchQuery) {
      where.name = {
        contains: searchQuery,
        mode: 'insensitive'
      };
    }

    // Build orderBy
    let orderBy: any = {};
    if (sortBy === 'size_desc') {
      orderBy = { members: { _count: 'desc' } };
    } else if (sortBy === 'size_asc') {
      orderBy = { members: { _count: 'asc' } };
    } else if (sortBy === 'name_asc') {
      orderBy = { name: 'asc' };
    } else if (sortBy === 'name_desc') {
      orderBy = { name: 'desc' };
    } else {
      orderBy = { createdAt: 'desc' }; // newest first
    }

    // Run count and findMany in parallel
    const [total, circles] = await Promise.all([
      prisma.circle.count({ where }),
      prisma.circle.findMany({
        where,
        take: limit,
        skip,
        orderBy,
        include: {
          organizer: {
            select: { id: true, email: true, firstName: true, lastName: true },
          },
          members: {
            include: {
              user: {
                select: { id: true, email: true, firstName: true, lastName: true },
              },
            },
          },
          contributions: {
            select: { amount: true },
          },
        },
      }),
    ]);

    return NextResponse.json(
      {
        data: circles,
        meta: {
          total,
          pages: Math.ceil(total / limit),
          currentPage: page,
        },
      },
      { status: 200 }
    );
  } catch (error) {
    console.error('List circles error:', error);
    return NextResponse.json({ error: 'Internal server error' }, { status: 500 });
  }
}