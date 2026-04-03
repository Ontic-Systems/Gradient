/*
 * Actor Scheduler Header
 *
 * Work-stealing thread pool scheduler for Gradient actors.
 * Features:
 *   - Fixed-size thread pool
 *   - Work-stealing queues for load balancing
 *   - FIFO per-actor message ordering
 *   - Cooperative scheduling with yield points
 */

#ifndef GRADIENT_SCHEDULER_H
#define GRADIENT_SCHEDULER_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <pthread.h>
#include "actor.h"

#ifdef __cplusplus
extern "C" {
#endif

/* ============================================================================
 * Configuration
 * ============================================================================ */

/* Default number of worker threads (0 = auto-detect CPU count) */
#define SCHEDULER_DEFAULT_THREADS 0

/* Default work queue capacity per thread */
#define SCHEDULER_QUEUE_CAPACITY 256

/* Maximum number of workers */
#define SCHEDULER_MAX_WORKERS 64

/* ============================================================================
 * Types
 * ============================================================================ */

/* Forward declarations */
typedef struct Scheduler Scheduler;
typedef struct WorkQueue WorkQueue;
typedef struct WorkStealQueue WorkStealQueue;

/* Work item: represents an actor ready to process a message */
typedef struct WorkItem {
    ActorId actor_id;       /* Actor to run */
    uint32_t priority;      /* Priority (lower = higher priority) */
    uint64_t enqueue_time;  /* Timestamp for queue metrics */
} WorkItem;

/* Work-stealing queue (Chase-Lev algorithm) */
typedef struct WorkStealQueue {
    volatile int64_t top;       /* Read index (steal from here) */
    volatile int64_t bottom;    /* Write index (owner pushes/pops here) */
    int64_t capacity;           /* Current array capacity */
    WorkItem* items;            /* Circular array of work items */
    void* reserved;             /* For memory alignment */
} WorkStealQueue;

/* Per-worker state */
typedef struct Worker {
    int id;                     /* Worker thread ID */
    pthread_t thread;           /* Worker thread handle */
    WorkStealQueue queue;       /* Work-stealing queue */
    Scheduler* scheduler;       /* Parent scheduler */
    volatile bool running;      /* Worker should keep running */
    uint64_t steal_attempts;    /* Statistics: steal attempts */
    uint64_t steal_successes; /* Statistics: successful steals */
    uint64_t tasks_executed;  /* Statistics: tasks executed */
    ActorId current_actor;    /* Currently executing actor */
} Worker;

/* Scheduler state */
typedef struct Scheduler {
    Worker* workers;            /* Array of worker threads */
    int num_workers;            /* Number of worker threads */
    volatile bool running;      /* Scheduler is active */
    volatile uint64_t total_actors;   /* Total spawned actors */
    volatile uint64_t active_actors;  /* Actors with pending work */
    
    /* Global queue for overflow/scheduling */
    pthread_mutex_t global_lock;
    WorkItem* global_queue;
    size_t global_head;
    size_t global_tail;
    size_t global_capacity;
    
    /* Parking/notification */
    pthread_mutex_t park_lock;
    pthread_cond_t park_cond;
    volatile int parked_workers;
} Scheduler;

/* ============================================================================
 * Scheduler Lifecycle
 * ============================================================================ */

/*
 * scheduler_create(num_threads) -> Scheduler*
 *
 * Create a new scheduler with the specified number of worker threads.
 * If num_threads is 0, auto-detect based on CPU count.
 * Returns NULL on allocation failure.
 */
Scheduler* scheduler_create(int num_threads);

/*
 * scheduler_destroy(scheduler) -> void
 *
 * Shutdown the scheduler and free all resources.
 * Waits for all workers to finish current tasks.
 */
void scheduler_destroy(Scheduler* scheduler);

/*
 * scheduler_start(scheduler) -> bool
 *
 * Start the scheduler's worker threads.
 * Returns true on success.
 */
bool scheduler_start(Scheduler* scheduler);

/*
 * scheduler_stop(scheduler) -> void
 *
 * Signal the scheduler to stop.
 * Workers will finish current tasks and exit.
 */
void scheduler_stop(Scheduler* scheduler);

/*
 * scheduler_wait_idle(scheduler, timeout_ms) -> bool
 *
 * Wait for all work to complete (scheduler becomes idle).
 * Returns true if idle, false if timeout.
 */
bool scheduler_wait_idle(Scheduler* scheduler, uint32_t timeout_ms);

/* ============================================================================
 * Work Management
 * ============================================================================ */

/*
 * scheduler_post_actor(scheduler, actor_id) -> bool
 *
 * Schedule an actor for execution.
 * Called when an actor receives a new message.
 * Returns true on success.
 */
bool scheduler_post_actor(Scheduler* scheduler, ActorId actor_id);

/*
 * scheduler_post_actor_to(scheduler, actor_id, worker_id) -> bool
 *
 * Schedule an actor to a specific worker (for affinity).
 */
bool scheduler_post_actor_to(Scheduler* scheduler, ActorId actor_id, int worker_id);

/*
 * scheduler_yield_current(worker) -> void
 *
 * Yield the current worker to allow other actors to run.
 */
void scheduler_yield_current(Worker* worker);

/* ============================================================================
 * Work-Stealing Queue Operations
 * ============================================================================ */

/*
 * wsqueue_init(queue, capacity) -> bool
 *
 * Initialize a work-stealing queue.
 */
bool wsqueue_init(WorkStealQueue* queue, size_t capacity);

/*
 * wsqueue_destroy(queue) -> void
 *
 * Free work-stealing queue resources.
 */
void wsqueue_destroy(WorkStealQueue* queue);

/*
 * wsqueue_push(queue, item) -> bool
 *
 * Push work item to the bottom of the queue (owner only).
 */
bool wsqueue_push(WorkStealQueue* queue, const WorkItem* item);

/*
 * wsqueue_pop(queue, out_item) -> bool
 *
 * Pop work item from the bottom of the queue (owner only).
 * Returns true if item was popped.
 */
bool wsqueue_pop(WorkStealQueue* queue, WorkItem* out_item);

/*
 * wsqueue_steal(queue, out_item) -> bool
 *
 * Steal work item from the top of the queue (other workers only).
 * Returns true if steal succeeded.
 */
bool wsqueue_steal(WorkStealQueue* queue, WorkItem* out_item);

/* ============================================================================
 * Global Queue Operations
 * ============================================================================ */

/*
 * global_queue_push(scheduler, item) -> bool
 *
 * Push to the global overflow queue.
 */
bool global_queue_push(Scheduler* scheduler, const WorkItem* item);

/*
 * global_queue_pop(scheduler, out_item) -> bool
 *
 * Pop from the global overflow queue.
 */
bool global_queue_pop(Scheduler* scheduler, WorkItem* out_item);

/* ============================================================================
 * Current Scheduler / Worker Access
 * ============================================================================ */

/*
 * scheduler_get_current() -> Scheduler*
 *
 * Get the scheduler for the current thread.
 * Returns NULL if not a worker thread.
 */
Scheduler* scheduler_get_current(void);

/*
 * worker_get_current() -> Worker*
 *
 * Get the worker structure for the current thread.
 * Returns NULL if not a worker thread.
 */
Worker* worker_get_current(void);

/*
 * worker_get_current_id() -> int
 *
 * Get the worker ID for the current thread.
 * Returns -1 if not a worker thread.
 */
int worker_get_current_id(void);

/* ============================================================================
 * Runtime Interface
 * ============================================================================ */

/*
 * _gradient_rt_scheduler_init(num_threads) -> int64_t
 *
 * Internal: Initialize the global scheduler.
 * Returns 1 on success, 0 on failure.
 */
int64_t _gradient_rt_scheduler_init(int64_t num_threads);

/*
 * _gradient_rt_scheduler_shutdown() -> void
 *
 * Internal: Shutdown the global scheduler.
 */
void _gradient_rt_scheduler_shutdown(void);

/*
 * _gradient_rt_scheduler_stats() -> void
 *
 * Internal: Print scheduler statistics to stderr.
 */
void _gradient_rt_scheduler_stats(void);

/*
 * scheduler_get_global() -> Scheduler*
 *
 * Get the global scheduler instance.
 * Returns NULL if not initialized.
 */
Scheduler* scheduler_get_global(void);

/* ============================================================================
 * Gradient Runtime Interface (for compiler integration)
 * ============================================================================ */

/*
 * __gradient_scheduler_init(num_threads) -> int64_t
 *
 * Initialize the global scheduler.
 * Returns 1 on success, 0 on failure.
 */
#define __gradient_scheduler_init _gradient_rt_scheduler_init

/*
 * __gradient_scheduler_shutdown() -> void
 *
 * Shutdown the global scheduler.
 */
#define __gradient_scheduler_shutdown _gradient_rt_scheduler_shutdown

/*
 * __gradient_scheduler_stats() -> void
 *
 * Print scheduler statistics to stderr.
 */
#define __gradient_scheduler_stats _gradient_rt_scheduler_stats

#ifdef __cplusplus
}
#endif

#endif /* GRADIENT_SCHEDULER_H */
