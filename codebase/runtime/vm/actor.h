/*
 * Actor Runtime Header
 *
 * Actor-based concurrency runtime for Gradient.
 * Each actor has:
 *   - Unique ID (ActorId)
 *   - Private state (void*)
 *   - Behavior table (message handlers)
 *   - Mailbox (bounded queue)
 *   - Private arena for memory management
 *
 * This follows the actor model: actors communicate via async message passing,
 * no shared memory between actors.
 */

#ifndef GRADIENT_ACTOR_H
#define GRADIENT_ACTOR_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include "../memory/arena.h"

#ifdef __cplusplus
extern "C" {
#endif

/* ============================================================================
 * Types
 * ============================================================================ */

/* Unique actor identifier */
typedef uint64_t ActorId;

/* Invalid/null actor ID */
#define ACTOR_ID_NULL ((ActorId)0)

/* Message type tag - identifies message handler */
typedef uint64_t MessageType;

/* Forward declarations */
typedef struct Actor Actor;
typedef struct Message Message;
typedef struct Mailbox Mailbox;

/* Behavior function: handles a message and updates actor state */
typedef void (*BehaviorFn)(Actor* self, const Message* msg);

/* Actor initialization function: called on spawn to set up initial state */
typedef void* (*ActorInitFn)(Arena* arena, size_t state_size);

/* ============================================================================
 * Message Structure
 * ============================================================================ */

/* Message sent between actors */
struct Message {
    ActorId sender;       /* Source actor ID */
    void* payload;        /* Message data (allocated in receiving actor's arena) */
    MessageType type;     /* Message type tag for dispatch */
    size_t payload_size;  /* Size of payload for copying */
};

/* ============================================================================
 * Mailbox Structure
 * ============================================================================ */

/* Bounded queue for actor mailbox with backpressure */
typedef struct Mailbox {
    Message* messages;      /* Ring buffer of messages */
    size_t capacity;        /* Maximum mailbox size */
    size_t head;            /* Read position */
    size_t tail;            /* Write position */
    size_t count;           /* Current message count */
    bool closed;            /* True if mailbox is closed (actor terminating) */
} Mailbox;

/* ============================================================================
 * Actor Structure
 * ============================================================================ */

/* Actor status states */
typedef enum ActorStatus {
    ACTOR_STATUS_IDLE = 0,      /* Actor waiting for messages */
    ACTOR_STATUS_RUNNING = 1,   /* Actor processing a message */
    ACTOR_STATUS_BLOCKED = 2,   /* Actor blocked on receive */
    ACTOR_STATUS_TERMINATING = 3, /* Actor shutting down */
    ACTOR_STATUS_DEAD = 4       /* Actor cleaned up */
} ActorStatus;

/* Actor instance */
struct Actor {
    ActorId id;             /* Unique actor ID */
    void* state;            /* Actor state (allocated in arena) */
    Arena* arena;           /* Per-actor memory arena */
    Mailbox mailbox;        /* Incoming message queue */
    ActorStatus status;     /* Current actor status */
    BehaviorFn* behaviors;  /* Behavior table (array indexed by message type) */
    size_t behavior_count;  /* Number of behaviors in table */
    void* scheduler_data;   /* Opaque pointer for scheduler use */
    int ref_count;          /* Reference count for cleanup */
};

/* ============================================================================
 * Mailbox Operations
 * ============================================================================ */

/*
 * mailbox_create(capacity) -> Mailbox
 *
 * Initialize a mailbox with the given capacity.
 * Returns a mailbox struct ready for use.
 */
Mailbox mailbox_create(size_t capacity);

/*
 * mailbox_destroy(mailbox) -> void
 *
 * Free all resources associated with a mailbox.
 * Does not free messages - caller must handle pending messages.
 */
void mailbox_destroy(Mailbox* mailbox);

/*
 * mailbox_send(mailbox, message) -> bool
 *
 * Enqueue a message into the mailbox.
 * Returns true if successful, false if mailbox is full (backpressure).
 * The message is copied into the mailbox.
 */
bool mailbox_send(Mailbox* mailbox, const Message* message);

/*
 * mailbox_receive(mailbox, out_message) -> bool
 *
 * Dequeue a message from the mailbox.
 * Returns true if a message was received, false if mailbox is empty.
 */
bool mailbox_receive(Mailbox* mailbox, Message* out_message);

/*
 * mailbox_try_receive(mailbox, out_message) -> bool
 *
 * Non-blocking receive. Returns immediately.
 * Returns true if message received, false if empty.
 */
bool mailbox_try_receive(Mailbox* mailbox, Message* out_message);

/*
 * mailbox_count(mailbox) -> size_t
 *
 * Get the current number of messages in the mailbox.
 */
size_t mailbox_count(const Mailbox* mailbox);

/*
 * mailbox_is_full(mailbox) -> bool
 *
 * Check if the mailbox is at capacity.
 */
bool mailbox_is_full(const Mailbox* mailbox);

/*
 * mailbox_close(mailbox) -> void
 *
 * Mark mailbox as closed (actor terminating).
 * No new messages can be sent after close.
 */
void mailbox_close(Mailbox* mailbox);

/* ============================================================================
 * Actor Operations
 * ============================================================================ */

/*
 * actor_create(id, state_size, behavior_count) -> Actor*
 *
 * Create a new actor with the given ID and allocate its arena.
 * state_size: bytes for actor state (allocated in arena)
 * behavior_count: size of behavior table
 * Returns NULL on allocation failure.
 */
Actor* actor_create(ActorId id, size_t state_size, size_t behavior_count);

/*
 * actor_destroy(actor) -> void
 *
 * Destroy an actor and free all associated resources.
 * This frees the arena (including state) and mailbox.
 */
void actor_destroy(Actor* actor);

/*
 * actor_set_behavior(actor, message_type, handler) -> bool
 *
 * Register a behavior handler for a message type.
 * Returns true on success, false if message_type >= behavior_count.
 */
bool actor_set_behavior(Actor* actor, MessageType type, BehaviorFn handler);

/*
 * actor_get_behavior(actor, message_type) -> BehaviorFn
 *
 * Get the behavior handler for a message type.
 * Returns NULL if no handler registered.
 */
BehaviorFn actor_get_behavior(const Actor* actor, MessageType type);

/*
 * actor_handle_message(actor, message) -> bool
 *
 * Dispatch a message to the appropriate behavior handler.
 * Returns true if handled, false if no handler exists.
 */
bool actor_handle_message(Actor* actor, const Message* message);

/*
 * actor_allocate(actor, size) -> void*
 *
 * Allocate memory from the actor's arena.
 * Returns NULL if allocation fails.
 */
void* actor_allocate(Actor* actor, size_t size);

/*
 * actor_allocate_aligned(actor, size, align) -> void*
 *
 * Allocate aligned memory from the actor's arena.
 */
void* actor_allocate_aligned(Actor* actor, size_t size, size_t align);

/* ============================================================================
 * Actor Runtime Interface (for compiler integration)
 * ============================================================================ */

/*
 * _gradient_rt_actor_spawn(initializer_fn, state_size) -> ActorId
 *
 * Internal: Create and spawn a new actor.
 * Returns the new actor's ID, or ACTOR_ID_NULL on failure.
 */
ActorId _gradient_rt_actor_spawn(ActorInitFn init_fn, size_t state_size);

/*
 * _gradient_rt_actor_send(target_id, message_type, payload, payload_size) -> int64_t
 *
 * Internal: Send a message to an actor.
 * The payload is copied into the target actor's arena.
 * Returns 1 on success, 0 on failure.
 */
int64_t _gradient_rt_actor_send(ActorId target_id, MessageType type, 
                                 const void* payload, size_t payload_size);

/*
 * _gradient_rt_actor_receive() -> Message*
 *
 * Internal: Block until a message is received by the current actor.
 * Returns a pointer to the message (in current actor's arena).
 * Returns NULL if the actor is terminated.
 */
Message* _gradient_rt_actor_receive(void);

/*
 * _gradient_rt_actor_try_receive() -> Message*
 *
 * Internal: Non-blocking receive. Returns immediately.
 * Returns a message if available, NULL if mailbox empty.
 */
Message* _gradient_rt_actor_try_receive(void);

/*
 * _gradient_rt_actor_yield() -> void
 *
 * Internal: Yield control to the scheduler, allowing other actors to run.
 */
void _gradient_rt_actor_yield(void);

/*
 * _gradient_rt_actor_self() -> ActorId
 *
 * Internal: Get the ID of the currently executing actor.
 * Returns ACTOR_ID_NULL if called outside actor context.
 */
ActorId _gradient_rt_actor_self(void);

/*
 * _gradient_rt_actor_terminate() -> void
 *
 * Internal: Terminate the current actor.
 * The actor will finish processing current message, then clean up.
 */
void _gradient_rt_actor_terminate(void);

/* ============================================================================
 * Gradient Runtime Interface (for compiler integration)
 * ============================================================================ */

/*
 * __gradient_actor_spawn(initializer_fn, state_size) -> ActorId
 *
 * Create and spawn a new actor.
 * Called by compiled Gradient code.
 * Returns the new actor's ID, or ACTOR_ID_NULL on failure.
 */
#define __gradient_actor_spawn _gradient_rt_actor_spawn

/*
 * __gradient_actor_send(target_id, message_type, payload, payload_size) -> int64_t
 *
 * Send a message to an actor.
 * The payload is copied into the target actor's arena.
 * Returns 1 on success, 0 on failure.
 */
#define __gradient_actor_send _gradient_rt_actor_send

/*
 * __gradient_actor_receive() -> Message*
 *
 * Block until a message is received by the current actor.
 * Returns a pointer to the message (in current actor's arena).
 * Returns NULL if the actor is terminated.
 */
#define __gradient_actor_receive _gradient_rt_actor_receive

/*
 * __gradient_actor_try_receive() -> Message*
 *
 * Non-blocking receive. Returns immediately.
 * Returns a message if available, NULL if mailbox empty.
 */
#define __gradient_actor_try_receive _gradient_rt_actor_try_receive

/*
 * __gradient_actor_yield() -> void
 *
 * Yield control to the scheduler, allowing other actors to run.
 * Called by compiled Gradient code for cooperative scheduling.
 */
#define __gradient_actor_yield _gradient_rt_actor_yield

/*
 * __gradient_actor_self() -> ActorId
 *
 * Get the ID of the currently executing actor.
 * Returns ACTOR_ID_NULL if called outside actor context.
 */
#define __gradient_actor_self _gradient_rt_actor_self

/*
 * __gradient_actor_terminate() -> void
 *
 * Terminate the current actor.
 * The actor will finish processing current message, then clean up.
 */
#define __gradient_actor_terminate _gradient_rt_actor_terminate

/* ============================================================================
 * Thread-Local Current Actor
 * ============================================================================ */

/*
 * actor_set_current(actor) -> void
 *
 * Set the current actor for this thread.
 * Called by scheduler when dispatching to an actor.
 */
void actor_set_current(Actor* actor);

/*
 * actor_get_current() -> Actor*
 *
 * Get the currently executing actor for this thread.
 * Returns NULL if not in actor context.
 */
Actor* actor_get_current(void);

/* ============================================================================
 * Utility Functions
 * ============================================================================ */

/*
 * actor_id_to_string(id, buffer, size) -> int
 *
 * Convert an ActorId to string representation.
 * Returns number of characters written, or -1 on error.
 */
int actor_id_to_string(ActorId id, char* buffer, size_t size);

#ifdef __cplusplus
}
#endif

#endif /* GRADIENT_ACTOR_H */
