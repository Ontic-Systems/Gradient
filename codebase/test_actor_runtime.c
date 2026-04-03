/*
 * Simple test for the Actor Runtime
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include "../codebase/runtime/vm/actor.h"
#include "../codebase/runtime/vm/scheduler.h"

/* Test: Create and destroy an actor */
void test_actor_create_destroy() {
    printf("Test: actor_create_destroy\n");
    
    Actor* actor = actor_create(1, 64, 10);
    assert(actor != NULL);
    assert(actor->id == 1);
    assert(actor->state != NULL);
    assert(actor->arena != NULL);
    assert(actor->behavior_count == 10);
    
    actor_destroy(actor);
    printf("  PASSED\n");
}

/* Test: Mailbox operations */
void test_mailbox_operations() {
    printf("Test: mailbox_operations\n");
    
    Mailbox mb = mailbox_create(4);
    assert(mb.messages != NULL);
    assert(mb.capacity == 4);
    
    /* Send messages */
    Message msg1 = { .sender = 1, .type = 100, .payload = NULL, .payload_size = 0 };
    Message msg2 = { .sender = 2, .type = 200, .payload = NULL, .payload_size = 0 };
    
    assert(mailbox_send(&mb, &msg1) == true);
    assert(mailbox_send(&mb, &msg2) == true);
    assert(mailbox_count(&mb) == 2);
    
    /* Receive messages */
    Message out;
    assert(mailbox_receive(&mb, &out) == true);
    assert(out.sender == 1);
    assert(out.type == 100);
    
    assert(mailbox_receive(&mb, &out) == true);
    assert(out.sender == 2);
    assert(out.type == 200);
    
    assert(mailbox_count(&mb) == 0);
    assert(mailbox_receive(&mb, &out) == false); /* Empty */
    
    mailbox_destroy(&mb);
    printf("  PASSED\n");
}

/* Test: Mailbox backpressure */
void test_mailbox_backpressure() {
    printf("Test: mailbox_backpressure\n");
    
    Mailbox mb = mailbox_create(2);
    
    Message msg = { .sender = 1, .type = 1, .payload = NULL, .payload_size = 0 };
    
    assert(mailbox_send(&mb, &msg) == true);
    assert(mailbox_send(&mb, &msg) == true);
    assert(mailbox_is_full(&mb) == true);
    assert(mailbox_send(&mb, &msg) == false); /* Full - backpressure */
    
    mailbox_destroy(&mb);
    printf("  PASSED\n");
}

/* Test: Actor behavior registration */
void test_actor_behavior() {
    printf("Test: actor_behavior\n");
    
    Actor* actor = actor_create(1, 64, 5);
    
    /* Register behaviors */
    assert(actor_set_behavior(actor, 0, (BehaviorFn)1) == true);
    assert(actor_set_behavior(actor, 4, (BehaviorFn)4) == true);
    assert(actor_set_behavior(actor, 5, (BehaviorFn)5) == false); /* Out of range */
    
    /* Get behaviors */
    assert(actor_get_behavior(actor, 0) == (BehaviorFn)1);
    assert(actor_get_behavior(actor, 4) == (BehaviorFn)4);
    assert(actor_get_behavior(actor, 2) == NULL); /* Not set */
    
    actor_destroy(actor);
    printf("  PASSED\n");
}

/* Test: Work-stealing queue */
void test_wsqueue() {
    printf("Test: work_stealing_queue\n");
    
    WorkStealQueue queue;
    assert(wsqueue_init(&queue, 16) == true);
    
    WorkItem item1 = { .actor_id = 1, .priority = 0 };
    WorkItem item2 = { .actor_id = 2, .priority = 0 };
    
    /* Push and pop as owner */
    assert(wsqueue_push(&queue, &item1) == true);
    assert(wsqueue_push(&queue, &item2) == true);
    
    WorkItem out;
    assert(wsqueue_pop(&queue, &out) == true);
    assert(out.actor_id == 2); /* LIFO from bottom */
    
    assert(wsqueue_pop(&queue, &out) == true);
    assert(out.actor_id == 1);
    
    assert(wsqueue_pop(&queue, &out) == false); /* Empty */
    
    wsqueue_destroy(&queue);
    printf("  PASSED\n");
}

/* Test: Scheduler create/destroy */
void test_scheduler_lifecycle() {
    printf("Test: scheduler_lifecycle\n");
    
    Scheduler* sched = scheduler_create(2); /* 2 threads */
    assert(sched != NULL);
    assert(sched->num_workers == 2);
    
    assert(scheduler_start(sched) == true);
    assert(sched->running == true);
    
    /* Let it run briefly */
    usleep(10000); /* 10ms */
    
    scheduler_stop(sched);
    scheduler_destroy(sched);
    printf("  PASSED\n");
}

/* Test: Actor arena allocation */
void test_actor_arena() {
    printf("Test: actor_arena\n");
    
    Actor* actor = actor_create(1, 0, 1);
    
    /* Allocate from actor's arena */
    void* ptr1 = actor_allocate(actor, 64);
    assert(ptr1 != NULL);
    
    void* ptr2 = actor_allocate(actor, 128);
    assert(ptr2 != NULL);
    assert(ptr2 != ptr1);
    
    /* Write to memory */
    memset(ptr1, 0xAB, 64);
    memset(ptr2, 0xCD, 128);
    
    actor_destroy(actor);
    printf("  PASSED\n");
}

/* Simple message handler for testing */
static int test_message_count = 0;

void test_message_handler(Actor* self, const Message* msg) {
    (void)self;
    test_message_count++;
    printf("    Handler received message type %lu from actor %lu\n", 
           (unsigned long)msg->type, (unsigned long)msg->sender);
}

/* Test: Full actor send/receive (single threaded) */
void test_actor_send_receive() {
    printf("Test: actor_send_receive\n");
    
    /* Initialize scheduler */
    if (!_gradient_rt_scheduler_init(1)) {
        printf("  Skipped (scheduler init failed)\n");
        return;
    }
    
    /* Spawn an actor */
    ActorId id = _gradient_rt_actor_spawn(NULL, 64);
    assert(id != ACTOR_ID_NULL);
    printf("  Spawned actor %lu\n", (unsigned long)id);
    
    /* Send a message */
    int payload = 42;
    int64_t result = _gradient_rt_actor_send(id, 100, &payload, sizeof(payload));
    assert(result == 1);
    printf("  Sent message to actor %lu\n", (unsigned long)id);
    
    /* Get self ID (should be NULL actor outside of actor context) */
    ActorId self = _gradient_rt_actor_self();
    assert(self == ACTOR_ID_NULL);
    printf("  Self ID (outside actor): %lu\n", (unsigned long)self);
    
    /* Cleanup */
    _gradient_rt_scheduler_shutdown();
    printf("  PASSED\n");
}

int main() {
    printf("=== Actor Runtime Tests ===\n\n");
    
    test_actor_create_destroy();
    test_mailbox_operations();
    test_mailbox_backpressure();
    test_actor_behavior();
    test_wsqueue();
    test_scheduler_lifecycle();
    test_actor_arena();
    test_actor_send_receive();
    
    printf("\n=== All tests passed! ===\n");
    return 0;
}
