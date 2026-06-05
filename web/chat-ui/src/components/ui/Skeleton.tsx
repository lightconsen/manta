interface SkeletonProps {
  className?: string;
}

export function Skeleton({ className = "" }: SkeletonProps) {
  return (
    <div
      className={`animate-pulse rounded-md bg-gray-200 dark:bg-neutral-700 ${className}`}
      aria-hidden="true"
    />
  );
}

export function MessageSkeleton() {
  return (
    <div className="py-4 px-4 sm:px-6 bg-gray-50/60 dark:bg-neutral-800/30">
      <div className="max-w-3xl mx-auto flex gap-3 flex-row">
        <div className="w-7 h-7 rounded-full shrink-0 bg-gray-200 dark:bg-neutral-700 animate-pulse" />
        <div className="flex-1 min-w-0 space-y-2 pt-1">
          <Skeleton className="h-3 w-16" />
          <Skeleton className="h-16 w-full rounded-xl" />
        </div>
      </div>
    </div>
  );
}
