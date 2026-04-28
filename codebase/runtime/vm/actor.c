/*
 * Actor Runtime Implementation
 *
 * Actor-based concurrency runtime for Gradient.
 * Provides mailbox-based message passing and actor lifecycle management.
 */

#include "actor.h"
#include "scheduler.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include <pthread.h>
#include <sched.h>

/* ============================================================================
 * Mailbox Implementation
 * ============================================================================ */

Mailbox mailbox_create(size_t capacity) {
    Mailbox mb;
    mb.messages = (Message*)calloc(capacity, sizeof(Message));
    mb.capacity = capacity;
    mb.head = 0;
    mb.tail = 0;
    mb.count = 0;
    mb.closed = false;
    /* GRA-179: serialize all mailbox mutations. */
    pthread_mutex_init(&mb.lock, NULL);
    return mb;
}

void mailbox_destroy(Mailbox* mailbox) {
    if (!mailbox) return;

    /* Free the ring buffer */
    free(mailbox->messages);
    mailbox->messages = NULL;
    mailbox->capacity = 0;
    mailbox->head = 0;
    mailbox->tail = 0;
    mailbox->count = 0;
    mailbox->closed = false;
    pthread_mutex_destroy(&mailbox->lock);
}

bool mailbox_send(Mailbox* mailbox, const Message* message) {
    if (!mailbox || !message) {
        return false;
    }

    pthread_mutex_lock(&mailbox->lock);

    if (mailbox->closed) {
        pthread_mutex_unlock(&mailbox->lock);
        return false;
    }

    /* Check for full mailbox (backpressure) */
    if (mailbox->count >= mailbox->capacity) {
        pthread_mutex_unlock(&mailbox->lock);
        return false;
    }

    /* Copy message into ring buffer */
    mailbox->messages[mailbox->tail] = *message;
    mailbox->tail = (mailbox->tail + 1) % mailbox->capacity;
    mailbox->count++;

    pthread_mutex_unlock(&mailbox->lock);
    return true;
}

bool mailbox_receive(Mailbox* mailbox, Message* out_message) {
    if (!mailbox || !out_message) {
        return false;
    }

    pthread_mutex_lock(&mailbox->lock);

    /* Check for empty mailbox */
    if (mailbox->count == 0) {
        pthread_mutex_unlock(&mailbox->lock);
        return false;
    }

    /* Copy message from ring buffer */
    *out_message = mailbox->messages[mailbox->head];
    mailbox->head = (mailbox->head + 1) % mailbox->capacity;
    mailbox->count--;

    pthread_mutex_unlock(&mailbox->lock);
    return true;
}

bool mailbox_try_receive(Mailbox* mailbox, Message* out_message) {
    /* Same as mailbox_receive - both are non-blocking at this level */
    return mailbox_receive(mailbox, out_message);
}

size_t mailbox_count(const Mailbox* mailbox) {
    if (!mailbox) return 0;
    /* Cast away const for the lock; the count itself is observed. */
    pthread_mutex_t* lk = (pthread_mutex_t*)&mailbox->lock;
    pthread_mutex_lock(lk);
    size_t c = mailbox->count;
    pthread_mutex_unlock(lk);
    return c;
}

bool mailbox_is_full(const Mailbox* mailbox) {
    if (!mailbox) return true;
    pthread_mutex_t* lk = (pthread_mutex_t*)&mailbox->lock;
    pthread_mutex_lock(lk);
    bool full = mailbox->count >= mailbox->capacity;
    pthread_mutex_unlock(lk);
    return full;
}

void mailbox_close(Mailbox* mailbox) {
    if (!mailbox) return;
    pthread_mutex_lock(&mailbox->lock);
    mailbox->closed = true;
    pthread_mutex_unlock(&mailbox->lock);
}

/* ============================================================================
 * Actor Implementation
 * ============================================================================ */

/* Default mailbox capacity */
#define DEFAULT_MAILBOX_CAPACITY 256

Actor* actor_create(ActorId id, size_t state_size, size_t behavior_count) {
    /* Allocate actor structure */
    Actor* actor = (Actor*)malloc(sizeof(Actor));
    if (!actor) return NULL;

    /* Initialize actor */
    actor->id = id;
    actor->status = ACTOR_STATUS_IDLE;
    actor->scheduler_data = NULL;
    atomic_store_explicit(&actor->ref_count, 1u, memory_order_release);

    /* GRA-179: arena_lock guards arena_alloc calls (cross-thread sends
     * allocate payloads in the receiver's arena) and the status field. */
    pthread_mutex_init(&actor->arena_lock, NULL);

    /* Create per-actor arena */
    actor->arena = arena_create();
    if (!actor->arena) {
        pthread_mutex_destroy(&actor->arena_lock);
        free(actor);
        return NULL;
    }
    
    /* Allocate actor state in arena */
    if (state_size > 0) {
        actor->state = arena_alloc(actor->arena, state_size);
        if (!actor->state) {
            arena_destroy(actor->arena);
            pthread_mutex_destroy(&actor->arena_lock);
            free(actor);
            return NULL;
        }
    } else {
        actor->state = NULL;
    }

    /* Create mailbox */
    actor->mailbox = mailbox_create(DEFAULT_MAILBOX_CAPACITY);
    if (!actor->mailbox.messages) {
        arena_destroy(actor->arena);
        pthread_mutex_destroy(&actor->arena_lock);
        free(actor);
        return NULL;
    }

    /* Allocate behavior table */
    actor->behavior_count = behavior_count;
    if (behavior_count > 0) {
        actor->behaviors = (BehaviorFn*)calloc(behavior_count, sizeof(BehaviorFn));
        if (!actor->behaviors) {
            mailbox_destroy(&actor->mailbox);
            arena_destroy(actor->arena);
            pthread_mutex_destroy(&actor->arena_lock);
            free(actor);
            return NULL;
        }
    } else {
        actor->behaviors = NULL;
    }

    return actor;
}

void actor_destroy(Actor* actor) {
    if (!actor) return;

    /* GRA-179: atomic refcount discipline. fetch_sub returns the value
     * BEFORE the decrement; only the thread that observes 1 -> 0 frees. */
    uint32_t prev = atomic_fetch_sub_explicit(&actor->ref_count, 1u,
                                              memory_order_acq_rel);
    if (prev > 1u) {
        return; /* Still referenced */
    }

    /* Set status to dead */
    actor->status = ACTOR_STATUS_DEAD;

    /* Clean up mailbox */
    mailbox_destroy(&actor->mailbox);

    /* Clean up arena (includes state) */
    if (actor->arena) {
        arena_destroy(actor->arena);
        actor->arena = NULL;
    }

    /* Free behavior table */
    free(actor->behaviors);
    actor->behaviors = NULL;

    pthread_mutex_destroy(&actor->arena_lock);

    /* Free actor structure */
    free(actor);
}

bool actor_set_behavior(Actor* actor, MessageType type, BehaviorFn handler) {
    if (!actor || !actor->behaviors || type >= actor->behavior_count) {
        return false;
    }
    
    actor->behaviors[type] = handler;
    return true;
}

BehaviorFn actor_get_behavior(const Actor* actor, MessageType type) {
    if (!actor || !actor->behaviors || type >= actor->behavior_count) {
        return NULL;
    }
    
    return actor->behaviors[type];
}

bool actor_handle_message(Actor* actor, const Message* message) {
    if (!actor || !message) {
        return false;
    }
    
    /* Get the behavior handler */
    BehaviorFn handler = actor_get_behavior(actor, message->type);
    if (!handler) {
        /* No handler for this message type */
        return false;
    }
    
    /* Update status and call handler */
    actor->status = ACTOR_STATUS_RUNNING;
    handler(actor, message);
    
    /* Return to idle if not terminating */
    if (actor->status != ACTOR_STATUS_TERMINATING && 
        actor->status != ACTOR_STATUS_DEAD) {
        actor->status = ACTOR_STATUS_IDLE;
    }
    
    return true;
}

void* actor_allocate(Actor* actor, size_t size) {
    if (!actor || !actor->arena) return NULL;
    /* GRA-179: cross-thread sends call this on a remote actor's arena. */
    pthread_mutex_lock(&actor->arena_lock);
    void* p = arena_alloc(actor->arena, size);
    pthread_mutex_unlock(&actor->arena_lock);
    return p;
}

void* actor_allocate_aligned(Actor* actor, size_t size, size_t align) {
    if (!actor || !actor->arena) return NULL;
    pthread_mutex_lock(&actor->arena_lock);
    void* p = arena_alloc_aligned(actor->arena, size, align);
    pthread_mutex_unlock(&actor->arena_lock);
    return p;
}

int actor_id_to_string(ActorId id, char* buffer, size_t size) {
    if (!buffer || size == 0) return -1;
    return snprintf(buffer, size, "Actor(%lu)", (unsigned long)id);
}

/* ============================================================================
 * Thread-Local Current Actor
 * ============================================================================ */

/* Thread-local storage for current actor */
static __thread Actor* tls_current_actor = NULL;

void actor_set_current(Actor* actor) {
    tls_current_actor = actor;
}

Actor* actor_get_current(void) {
    return tls_current_actor;
}

/* ============================================================================
 * Actor Registry (for looking up actors by ID)
 * ============================================================================ */

/* Simple hash map for actor lookup.
 *
 * GRA-179: a removed slot is replaced with TOMBSTONE rather than NULL so that
 * subsequent linear-probe lookups don't terminate prematurely. NULL means
 * "never used"; TOMBSTONE means "previously occupied, keep probing". Inserts
 * may reuse tombstone slots.
 */
#define REGISTRY_CAPACITY 1024
#define ACTOR_REGISTRY_TOMBSTONE ((Actor*)(uintptr_t)0x1)

static struct {
    pthread_mutex_t lock;
    Actor* actors[REGISTRY_CAPACITY];
    ActorId next_id;
    bool initialized;
} actor_registry = {
    .lock = PTHREAD_MUTEX_INITIALIZER,
    .next_id = 1,
    .initialized = false
};

static void actor_registry_init(void) {
    if (!actor_registry.initialized) {
        memset(actor_registry.actors, 0, sizeof(actor_registry.actors));
        actor_registry.initialized = true;
    }
}

static ActorId actor_registry_add(Actor* actor) {
    actor_registry_init();

    pthread_mutex_lock(&actor_registry.lock);

    ActorId id = actor_registry.next_id++;
    if (id == ACTOR_ID_NULL) {
        id = actor_registry.next_id++; /* Skip NULL id */
    }

    size_t idx = id % REGISTRY_CAPACITY;

    /* Linear probing - reuse NULL or TOMBSTONE slots. */
    for (size_t i = 0; i < REGISTRY_CAPACITY; i++) {
        size_t pos = (idx + i) % REGISTRY_CAPACITY;
        Actor* slot = actor_registry.actors[pos];
        if (slot == NULL || slot == ACTOR_REGISTRY_TOMBSTONE) {
            actor_registry.actors[pos] = actor;
            actor->id = id;
            pthread_mutex_unlock(&actor_registry.lock);
            return id;
        }
    }

    pthread_mutex_unlock(&actor_registry.lock);
    return ACTOR_ID_NULL; /* Registry full */
}

static Actor* actor_registry_lookup(ActorId id) {
    if (id == ACTOR_ID_NULL) return NULL;

    actor_registry_init();

    pthread_mutex_lock(&actor_registry.lock);

    size_t idx = id % REGISTRY_CAPACITY;
    Actor* found = NULL;

    for (size_t i = 0; i < REGISTRY_CAPACITY; i++) {
        size_t pos = (idx + i) % REGISTRY_CAPACITY;
        Actor* slot = actor_registry.actors[pos];
        if (slot == NULL) {
            break; /* Genuinely empty: definitely not in table */
        }
        if (slot == ACTOR_REGISTRY_TOMBSTONE) {
            continue; /* GRA-179: keep probing past tombstones */
        }
        if (slot->id == id) {
            found = slot;
            /* GRA-179: atomic increment under registry lock pins the actor
             * against concurrent destruction by a parallel remove. */
            atomic_fetch_add_explicit(&found->ref_count, 1u,
                                      memory_order_acq_rel);
            break;
        }
    }

    pthread_mutex_unlock(&actor_registry.lock);
    return found;
}

static void actor_registry_remove(ActorId id) {
    if (id == ACTOR_ID_NULL) return;

    actor_registry_init();

    Actor* to_release = NULL;

    pthread_mutex_lock(&actor_registry.lock);

    size_t idx = id % REGISTRY_CAPACITY;

    for (size_t i = 0; i < REGISTRY_CAPACITY; i++) {
        size_t pos = (idx + i) % REGISTRY_CAPACITY;
        Actor* slot = actor_registry.actors[pos];
        if (slot == NULL) {
            break;
        }
        if (slot == ACTOR_REGISTRY_TOMBSTONE) {
            continue;
        }
        if (slot->id == id) {
            /* GRA-179: replace with TOMBSTONE, not NULL, so later
             * lookups don't short-circuit on this slot. */
            actor_registry.actors[pos] = ACTOR_REGISTRY_TOMBSTONE;
            to_release = slot;
            break;
        }
    }

    pthread_mutex_unlock(&actor_registry.lock);

    /* Drop the registry's own reference outside the registry lock; this may
     * destroy the actor if no senders are still holding a lookup ref. */
    if (to_release) {
        actor_destroy(to_release);
    }
}

/* ============================================================================
 * Gradient Runtime Interface
 * ============================================================================ */

ActorId _gradient_rt_actor_spawn(ActorInitFn init_fn, size_t state_size) {
    /* Create new actor */
    Actor* actor = actor_create(ACTOR_ID_NULL, state_size, 64); /* Default 64 behaviors */
    if (!actor) return ACTOR_ID_NULL;

    /* Register in registry to get ID */
    ActorId id = actor_registry_add(actor);
    if (id == ACTOR_ID_NULL) {
        actor_destroy(actor);
        return ACTOR_ID_NULL;
    }

    /* Initialize state with user function if provided */
    if (init_fn) {
        /* init_fn allocates from the actor's own arena - guard it. */
        pthread_mutex_lock(&actor->arena_lock);
        void* state = init_fn(actor->arena, state_size);
        pthread_mutex_unlock(&actor->arena_lock);
        /* GRA-179: only memcpy when we have a destination, a source, and
         * a non-zero size. Previously memcpy(actor->state, state, 0) ran
         * even when state_size==0 or state==NULL, which is undefined
         * behavior per C11 7.1.4 / 7.24.2.1. */
        if (state && state != actor->state &&
            actor->state != NULL && state_size > 0) {
            memcpy(actor->state, state, state_size);
        }
    }

    /* Notify scheduler of new actor (if scheduler is active) */
    /* This will be handled by scheduler_post_actor */

    return id;
}

int64_t _gradient_rt_actor_send(ActorId target_id, MessageType type,
                                 const void* payload, size_t payload_size) {
    /* Look up target actor (lookup pins ref_count under registry lock) */
    Actor* target = actor_registry_lookup(target_id);
    if (!target) return 0;

    /* GRA-179: allocate payload in target's arena under arena_lock so that
     * concurrent senders don't corrupt the bump pointer. */
    void* payload_copy = NULL;
    if (payload && payload_size > 0) {
        payload_copy = actor_allocate(target, payload_size);
        if (!payload_copy) {
            actor_destroy(target); /* atomic refcount release */
            return 0;
        }
        memcpy(payload_copy, payload, payload_size);
    }

    /* Create message */
    Message msg;
    Actor* current = actor_get_current();
    msg.sender = current ? current->id : ACTOR_ID_NULL;
    msg.payload = payload_copy;
    msg.type = type;
    msg.payload_size = payload_size;

    /* Send to mailbox (now thread-safe under mailbox->lock). */
    bool success = mailbox_send(&target->mailbox, &msg);

    if (success) {
        /* Notify scheduler that actor has work */
        Scheduler* sched = scheduler_get_global();
        if (sched) {
            scheduler_post_actor(sched, target_id);
        }
    }

    /* GRA-179: drop the lookup reference via the atomic refcount path. */
    actor_destroy(target);
    return success ? 1 : 0;
}

Message* _gradient_rt_actor_receive(void) {
    Actor* current = actor_get_current();
    if (!current) return NULL;

    /* GRA-179: this allocation races with senders writing to the same arena. */
    Message* msg = (Message*)actor_allocate(current, sizeof(Message));
    if (!msg) return NULL;

    while (true) {
        if (mailbox_receive(&current->mailbox, msg)) {
            return msg;
        }

        /* Check if actor is terminating */
        if (current->status == ACTOR_STATUS_TERMINATING ||
            current->mailbox.closed) {
            return NULL;
        }

        /* Yield to scheduler */
        _gradient_rt_actor_yield();
    }
}

Message* _gradient_rt_actor_try_receive(void) {
    Actor* current = actor_get_current();
    if (!current) return NULL;

    /* GRA-179: same arena-lock concern as _gradient_rt_actor_receive. */
    Message* msg = (Message*)actor_allocate(current, sizeof(Message));
    if (!msg) return NULL;

    if (mailbox_receive(&current->mailbox, msg)) {
        return msg;
    }

    return NULL;
}

void _gradient_rt_actor_yield(void) {
    /* Yield to scheduler - use scheduler's yield if available */
    Worker* worker = worker_get_current();
    if (worker) {
        scheduler_yield_current(worker);
    } else {
        sched_yield();
    }
}

ActorId _gradient_rt_actor_self(void) {
    Actor* current = actor_get_current();
    return current ? current->id : ACTOR_ID_NULL;
}

void _gradient_rt_actor_terminate(void) {
    Actor* current = actor_get_current();
    if (!current) return;
    
    current->status = ACTOR_STATUS_TERMINATING;
    mailbox_close(&current->mailbox);
    
    /* Remove from registry */
    actor_registry_remove(current->id);
    
    /* Actor will be destroyed after current message processing completes */
}
