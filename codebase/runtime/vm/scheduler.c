/*
 * Actor Scheduler Implementation
 *
 * Work-stealing thread pool scheduler for Gradient actors.
 * Implements the Chase-Lev work-stealing algorithm.
 */

#include "scheduler.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include <pthread.h>
#include <unistd.h>
#include <time.h>
#include <sched.h>
#include <sys/sysinfo.h>

/* ============================================================================
 * Thread-Local Storage
 * ============================================================================ */

static __thread Worker* current_worker = NULL;
static __thread Scheduler* current_scheduler = NULL;

Scheduler* scheduler_get_current(void) {
    return current_scheduler;
}

Worker* worker_get_current(void) {
    return current_worker;
}

int worker_get_current_id(void) {
    return current_worker ? current_worker->id : -1;
}

/* ============================================================================
 * Work-Stealing Queue (Chase-Lev Algorithm)
 * ============================================================================ */

/* Minimum queue capacity (must be power of 2) */
#define WSQUEUE_MIN_CAPACITY 16

/* Grow factor for queue resizing */
#define WSQUEUE_GROW_FACTOR 2

bool wsqueue_init(WorkStealQueue* queue, size_t capacity) {
    if (!queue) return false;
    
    /* Round up to power of 2 */
    size_t cap = WSQUEUE_MIN_CAPACITY;
    while (cap < capacity) {
        cap *= 2;
    }
    
    queue->items = (WorkItem*)calloc(cap, sizeof(WorkItem));
    if (!queue->items) return false;
    
    queue->capacity = (int64_t)cap;
    queue->top = 0;
    queue->bottom = 0;
    
    return true;
}

void wsqueue_destroy(WorkStealQueue* queue) {
    if (!queue) return;
    free(queue->items);
    queue->items = NULL;
    queue->capacity = 0;
    queue->top = 0;
    queue->bottom = 0;
}

bool wsqueue_push(WorkStealQueue* queue, const WorkItem* item) {
    if (!queue || !item) return false;
    
    int64_t b = queue->bottom;
    int64_t t = queue->top;
    int64_t size = b - t;
    
    /* Check if queue is full */
    if ((int64_t)size >= queue->capacity - 1) {
        /* Queue full - could grow here, but for simplicity we fail */
        return false;
    }
    
    /* Store item at bottom */
    int64_t mask = queue->capacity - 1;
    queue->items[b & mask] = *item;
    
    /* Memory barrier to ensure item is written before bottom is updated */
    __sync_synchronize();
    
    queue->bottom = b + 1;
    
    return true;
}

bool wsqueue_pop(WorkStealQueue* queue, WorkItem* out_item) {
    if (!queue || !out_item) return false;
    
    int64_t b = queue->bottom - 1;
    queue->bottom = b;
    
    /* Memory barrier */
    __sync_synchronize();
    
    int64_t t = queue->top;
    int64_t size = b - t;
    
    if (size < 0) {
        /* Queue empty */
        queue->bottom = t;
        return false;
    }
    
    /* Pop from bottom */
    int64_t mask = queue->capacity - 1;
    *out_item = queue->items[b & mask];
    
    if (size == 0) {
        /* Last item - race with stealers */
        if (!__sync_bool_compare_and_swap(&queue->top, t, t + 1)) {
            /* Lost the race - stealer got it */
            queue->bottom = t + 1;
            return false;
        }
        queue->bottom = t + 1;
    }
    
    return true;
}

bool wsqueue_steal(WorkStealQueue* queue, WorkItem* out_item) {
    if (!queue || !out_item) return false;
    
    int64_t t = queue->top;
    
    /* Memory barrier */
    __sync_synchronize();
    
    int64_t b = queue->bottom;
    int64_t size = b - t;
    
    if (size <= 0) {
        return false; /* Queue empty */
    }
    
    /* Try to steal from top */
    int64_t mask = queue->capacity - 1;
    *out_item = queue->items[t & mask];
    
    if (!__sync_bool_compare_and_swap(&queue->top, t, t + 1)) {
        /* Lost the race */
        return false;
    }
    
    return true;
}

/* ============================================================================
 * Global Queue
 * ============================================================================ */

static bool global_queue_init(Scheduler* sched, size_t capacity) {
    pthread_mutex_init(&sched->global_lock, NULL);
    sched->global_queue = (WorkItem*)calloc(capacity, sizeof(WorkItem));
    if (!sched->global_queue) return false;
    sched->global_capacity = capacity;
    sched->global_head = 0;
    sched->global_tail = 0;
    return true;
}

static void global_queue_destroy(Scheduler* sched) {
    if (!sched) return;
    pthread_mutex_destroy(&sched->global_lock);
    free(sched->global_queue);
    sched->global_queue = NULL;
}

bool global_queue_push(Scheduler* sched, const WorkItem* item) {
    if (!sched || !item) return false;
    
    pthread_mutex_lock(&sched->global_lock);
    
    size_t size = (sched->global_tail - sched->global_head + sched->global_capacity) 
                   % sched->global_capacity;
    
    if (size >= sched->global_capacity - 1) {
        pthread_mutex_unlock(&sched->global_lock);
        return false; /* Full */
    }
    
    sched->global_queue[sched->global_tail] = *item;
    sched->global_tail = (sched->global_tail + 1) % sched->global_capacity;
    
    pthread_mutex_unlock(&sched->global_lock);
    return true;
}

bool global_queue_pop(Scheduler* sched, WorkItem* out_item) {
    if (!sched || !out_item) return false;
    
    pthread_mutex_lock(&sched->global_lock);
    
    if (sched->global_head == sched->global_tail) {
        pthread_mutex_unlock(&sched->global_lock);
        return false; /* Empty */
    }
    
    *out_item = sched->global_queue[sched->global_head];
    sched->global_head = (sched->global_head + 1) % sched->global_capacity;
    
    pthread_mutex_unlock(&sched->global_lock);
    return true;
}

/* ============================================================================
 * Worker Thread
 * ============================================================================ */

/* Forward declaration */
static void* worker_thread(void* arg);

/* Execute an actor's pending messages */
static void worker_execute_actor(Worker* worker, ActorId actor_id) {
    /* Look up actor */
    /* This is a simplified version - real implementation needs actor registry */
    /* For now, we rely on the actor being in thread-local storage */
    
    Actor* actor = actor_get_current();
    if (!actor || actor->id != actor_id) {
        /* Actor not available on this thread */
        /* In full implementation, would look up in global registry */
        return;
    }
    
    worker->current_actor = actor_id;
    
    /* Process all pending messages in the mailbox */
    Message msg;
    while (mailbox_try_receive(&actor->mailbox, &msg)) {
        actor_handle_message(actor, &msg);
        
        /* Check for yield/termination */
        if (actor->status == ACTOR_STATUS_TERMINATING) {
            break;
        }
    }
    
    /* Update actor status */
    if (actor->status == ACTOR_STATUS_TERMINATING) {
        actor->status = ACTOR_STATUS_DEAD;
        actor_destroy(actor);
    } else if (mailbox_count(&actor->mailbox) > 0) {
        /* More messages arrived - reschedule */
        WorkItem item = {
            .actor_id = actor_id,
            .priority = 0,
            .enqueue_time = 0
        };
        wsqueue_push(&worker->queue, &item);
    } else {
        actor->status = ACTOR_STATUS_IDLE;
    }
    
    worker->current_actor = ACTOR_ID_NULL;
    worker->tasks_executed++;
}

/* Steal work from another worker */
static bool worker_steal(Worker* worker, WorkItem* out_item) {
    Scheduler* sched = worker->scheduler;
    int num_workers = sched->num_workers;
    
    /* Try to steal from each other worker */
    for (int i = 0; i < num_workers; i++) {
        if (i == worker->id) continue;
        
        Worker* victim = &sched->workers[i];
        worker->steal_attempts++;
        
        if (wsqueue_steal(&victim->queue, out_item)) {
            worker->steal_successes++;
            return true;
        }
    }
    
    return false;
}

/* Main worker loop */
static void* worker_thread(void* arg) {
    Worker* worker = (Worker*)arg;
    Scheduler* sched = worker->scheduler;
    
    /* Set thread-local storage */
    current_worker = worker;
    current_scheduler = sched;
    
    /* Run until scheduler stops */
    while (sched->running) {
        WorkItem item;
        bool got_work = false;
        
        /* 1. Try to pop from own queue */
        if (wsqueue_pop(&worker->queue, &item)) {
            got_work = true;
        }
        
        /* 2. Try to steal from other workers */
        if (!got_work) {
            if (worker_steal(worker, &item)) {
                got_work = true;
            }
        }
        
        /* 3. Try global queue */
        if (!got_work) {
            if (global_queue_pop(sched, &item)) {
                got_work = true;
            }
        }
        
        /* 4. Execute work or park */
        if (got_work) {
            worker_execute_actor(worker, item.actor_id);
        } else {
            /* No work available - park briefly */
            pthread_mutex_lock(&sched->park_lock);
            sched->parked_workers++;
            struct timespec ts;
            clock_gettime(CLOCK_REALTIME, &ts);
            ts.tv_nsec += 1000000; /* 1ms */
            if (ts.tv_nsec >= 1000000000) {
                ts.tv_sec++;
                ts.tv_nsec -= 1000000000;
            }
            pthread_cond_timedwait(&sched->park_cond, &sched->park_lock, &ts);
            sched->parked_workers--;
            pthread_mutex_unlock(&sched->park_lock);
        }
    }
    
    current_worker = NULL;
    current_scheduler = NULL;
    return NULL;
}

void scheduler_yield_current(Worker* worker) {
    (void)worker; /* Unused for now */
    
    /* Yield to scheduler - allow other actors to run */
    /* In a more advanced implementation, this could trigger work-stealing */
    sched_yield();
}

/* ============================================================================
 * Scheduler Lifecycle
 * ============================================================================ */

static int get_cpu_count(void) {
    return get_nprocs();
}

Scheduler* scheduler_create(int num_threads) {
    if (num_threads <= 0) {
        num_threads = get_cpu_count();
    }
    
    if (num_threads > SCHEDULER_MAX_WORKERS) {
        num_threads = SCHEDULER_MAX_WORKERS;
    }
    
    Scheduler* sched = (Scheduler*)calloc(1, sizeof(Scheduler));
    if (!sched) return NULL;
    
    sched->num_workers = num_threads;
    sched->running = false;
    sched->total_actors = 0;
    sched->active_actors = 0;
    
    /* Initialize global queue */
    if (!global_queue_init(sched, SCHEDULER_QUEUE_CAPACITY * 2)) {
        free(sched);
        return NULL;
    }
    
    /* Initialize parking */
    pthread_mutex_init(&sched->park_lock, NULL);
    pthread_cond_init(&sched->park_cond, NULL);
    sched->parked_workers = 0;
    
    /* Allocate workers */
    sched->workers = (Worker*)calloc(num_threads, sizeof(Worker));
    if (!sched->workers) {
        global_queue_destroy(sched);
        free(sched);
        return NULL;
    }
    
    /* Initialize each worker */
    for (int i = 0; i < num_threads; i++) {
        Worker* w = &sched->workers[i];
        w->id = i;
        w->scheduler = sched;
        w->running = false;
        w->steal_attempts = 0;
        w->steal_successes = 0;
        w->tasks_executed = 0;
        w->current_actor = ACTOR_ID_NULL;
        
        if (!wsqueue_init(&w->queue, SCHEDULER_QUEUE_CAPACITY)) {
            /* Cleanup */
            for (int j = 0; j < i; j++) {
                wsqueue_destroy(&sched->workers[j].queue);
            }
            free(sched->workers);
            global_queue_destroy(sched);
            free(sched);
            return NULL;
        }
    }
    
    return sched;
}

void scheduler_destroy(Scheduler* sched) {
    if (!sched) return;
    
    /* Stop scheduler */
    scheduler_stop(sched);
    
    /* Wait for workers to finish */
    for (int i = 0; i < sched->num_workers; i++) {
        pthread_join(sched->workers[i].thread, NULL);
    }
    
    /* Cleanup workers */
    for (int i = 0; i < sched->num_workers; i++) {
        wsqueue_destroy(&sched->workers[i].queue);
    }
    
    pthread_mutex_destroy(&sched->park_lock);
    pthread_cond_destroy(&sched->park_cond);
    
    free(sched->workers);
    global_queue_destroy(sched);
    free(sched);
}

bool scheduler_start(Scheduler* sched) {
    if (!sched || sched->running) return false;
    
    sched->running = true;
    
    /* Start worker threads */
    for (int i = 0; i < sched->num_workers; i++) {
        Worker* w = &sched->workers[i];
        w->running = true;
        
        if (pthread_create(&w->thread, NULL, worker_thread, w) != 0) {
            /* Cleanup on failure */
            sched->running = false;
            for (int j = 0; j < i; j++) {
                pthread_join(sched->workers[j].thread, NULL);
            }
            return false;
        }
    }
    
    return true;
}

void scheduler_stop(Scheduler* sched) {
    if (!sched) return;
    sched->running = false;
    
    /* Wake up parked workers */
    pthread_mutex_lock(&sched->park_lock);
    pthread_cond_broadcast(&sched->park_cond);
    pthread_mutex_unlock(&sched->park_lock);
}

bool scheduler_wait_idle(Scheduler* sched, uint32_t timeout_ms) {
    if (!sched) return false;
    
    /* Simple implementation: poll until idle or timeout */
    struct timespec start;
    clock_gettime(CLOCK_MONOTONIC, &start);
    
    while (sched->running) {
        /* Check if all queues are empty */
        bool all_empty = true;
        
        pthread_mutex_lock(&sched->global_lock);
        if (sched->global_head != sched->global_tail) {
            all_empty = false;
        }
        pthread_mutex_unlock(&sched->global_lock);
        
        if (all_empty) {
            for (int i = 0; i < sched->num_workers; i++) {
                if (sched->workers[i].queue.bottom != sched->workers[i].queue.top) {
                    all_empty = false;
                    break;
                }
            }
        }
        
        if (all_empty && sched->parked_workers == sched->num_workers) {
            return true;
        }
        
        /* Check timeout */
        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        uint64_t elapsed_ms = (now.tv_sec - start.tv_sec) * 1000 + 
                              (now.tv_nsec - start.tv_nsec) / 1000000;
        if (elapsed_ms >= timeout_ms) {
            return false;
        }
        
        /* Sleep briefly */
        usleep(1000); /* 1ms */
    }
    
    return false;
}

/* ============================================================================
 * Work Posting
 * ============================================================================ */

bool scheduler_post_actor(Scheduler* sched, ActorId actor_id) {
    if (!sched || !sched->running) return false;
    
    /* Get current worker if any */
    Worker* current = worker_get_current();
    
    WorkItem item = {
        .actor_id = actor_id,
        .priority = 0,
        .enqueue_time = 0 /* Could use actual timestamp */
    };
    
    if (current) {
        /* Push to current worker's queue (likely same actor) */
        if (wsqueue_push(&current->queue, &item)) {
            return true;
        }
    }
    
    /* Try to push to a random worker's queue */
    int worker_id = rand() % sched->num_workers;
    Worker* w = &sched->workers[worker_id];
    if (wsqueue_push(&w->queue, &item)) {
        return true;
    }
    
    /* Fall back to global queue */
    if (global_queue_push(sched, &item)) {
        /* Wake up a parked worker */
        pthread_mutex_lock(&sched->park_lock);
        pthread_cond_signal(&sched->park_cond);
        pthread_mutex_unlock(&sched->park_lock);
        return true;
    }
    
    return false;
}

bool scheduler_post_actor_to(Scheduler* sched, ActorId actor_id, int worker_id) {
    if (!sched || !sched->running) return false;
    if (worker_id < 0 || worker_id >= sched->num_workers) return false;
    
    WorkItem item = {
        .actor_id = actor_id,
        .priority = 0,
        .enqueue_time = 0
    };
    
    Worker* w = &sched->workers[worker_id];
    if (wsqueue_push(&w->queue, &item)) {
        return true;
    }
    
    /* Fall back to global queue */
    return global_queue_push(sched, &item);
}

/* ============================================================================
 * Runtime Interface
 * ============================================================================ */

/* Global scheduler instance */
static Scheduler* global_scheduler = NULL;
static pthread_mutex_t global_scheduler_lock = PTHREAD_MUTEX_INITIALIZER;

int64_t _gradient_rt_scheduler_init(int64_t num_threads) {
    pthread_mutex_lock(&global_scheduler_lock);
    
    if (global_scheduler != NULL) {
        pthread_mutex_unlock(&global_scheduler_lock);
        return 1; /* Already initialized */
    }
    
    int threads = (int)num_threads;
    if (threads < 0) threads = 0;
    
    global_scheduler = scheduler_create(threads);
    if (!global_scheduler) {
        pthread_mutex_unlock(&global_scheduler_lock);
        return 0;
    }
    
    if (!scheduler_start(global_scheduler)) {
        scheduler_destroy(global_scheduler);
        global_scheduler = NULL;
        pthread_mutex_unlock(&global_scheduler_lock);
        return 0;
    }
    
    pthread_mutex_unlock(&global_scheduler_lock);
    return 1;
}

void _gradient_rt_scheduler_shutdown(void) {
    pthread_mutex_lock(&global_scheduler_lock);
    
    if (global_scheduler) {
        scheduler_destroy(global_scheduler);
        global_scheduler = NULL;
    }
    
    pthread_mutex_unlock(&global_scheduler_lock);
}

void _gradient_rt_scheduler_stats(void) {
    pthread_mutex_lock(&global_scheduler_lock);
    
    if (!global_scheduler) {
        fprintf(stderr, "Scheduler: not initialized\n");
        pthread_mutex_unlock(&global_scheduler_lock);
        return;
    }
    
    Scheduler* sched = global_scheduler;
    
    fprintf(stderr, "=== Scheduler Statistics ===\n");
    fprintf(stderr, "Workers: %d\n", sched->num_workers);
    fprintf(stderr, "Running: %s\n", sched->running ? "yes" : "no");
    fprintf(stderr, "Total actors: %lu\n", (unsigned long)sched->total_actors);
    fprintf(stderr, "Active actors: %lu\n", (unsigned long)sched->active_actors);
    fprintf(stderr, "Parked workers: %d\n", sched->parked_workers);
    
    for (int i = 0; i < sched->num_workers; i++) {
        Worker* w = &sched->workers[i];
        fprintf(stderr, "\nWorker %d:\n", i);
        fprintf(stderr, "  Tasks executed: %lu\n", (unsigned long)w->tasks_executed);
        fprintf(stderr, "  Steal attempts: %lu\n", (unsigned long)w->steal_attempts);
        fprintf(stderr, "  Steal successes: %lu\n", (unsigned long)w->steal_successes);
        fprintf(stderr, "  Queue size: %ld\n", 
                (long)(w->queue.bottom - w->queue.top));
    }
    
    fprintf(stderr, "============================\n");
    
    pthread_mutex_unlock(&global_scheduler_lock);
}

/* Get global scheduler for actor integration */
Scheduler* scheduler_get_global(void) {
    return global_scheduler;
}
