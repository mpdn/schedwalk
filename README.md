# schedwalk

Test futures under all possible polling schedules.

Concurrent systems are hard. It can be very easy to accidentally assume progress happens in some
specific order (i.e. race conditions). For async systems in Rust, that might be an assumption on the
order that futures are polled - an assumption of the polling schedule.

Most async test runtimes only executes one schedule for your test and will never cover all possible
schedules. `schedwalk` is a an async test harness that allows you to reliably test all possible
schedules.


## Example

Suppose we are developing a web application and want to compute the average response time. We might
model that with two tasks like this:

```
# use std::convert::identity as spawn;
# futures::executor::block_on(async {
use futures::{channel::mpsc, join};

let (sender, mut receiver) = mpsc::unbounded::<u32>();

let send_task = spawn(async move {
    sender.unbounded_send(23).unwrap();
    sender.unbounded_send(20).unwrap();
    sender.unbounded_send(54).unwrap();
});

let avg_task = spawn(async move {
    let mut sum = 0;
    let mut count = 0;
    while let Some(num) = receiver.try_next().unwrap() {
        sum += num;
        count += 1;
    }

    println!("average is {}", sum / count)
});

join!(send_task, avg_task);
# })
```

But this has a race condition bug. What if `avg_task` executes before `send_task`? Then `count` will
be 0 and we will thus divide by 0! We have implicitly assumed one task executes before the other.

So how can we have create a test that trigger the above race condition? We could try executing under
an async runtime like Tokio, but the problem with this is that it does not actually guarantee that
the failing schedule will be executed. And in fact, at time of writing, it seems that the single
threaded executor *never* triggers the failure. Using the multithreaded executor *may* trigger the
failure, but there is no guarantee of that. At best, we have created a flaky test.

Ideally, we want to test such code in a way where we fail deterministically every time in case of
such bugs.

Enter `schedwalk`: a library for testing futures under all possible schedules. Using `schedwalk` we
can create a test like this:

```should_panic
use schedwalk::{for_all_schedules, spawn};
use futures::{channel::mpsc, join};

for_all_schedules(|| async {
    let (sender, mut receiver) = mpsc::unbounded::<u32>();

    let send_task = spawn(async move {
        sender.unbounded_send(23).unwrap();
        sender.unbounded_send(20).unwrap();
        sender.unbounded_send(54).unwrap();
    });

    let avg_task = spawn(async move {
        let mut sum = 0;
        let mut count = 0;
        while let Some(num) = receiver.try_next().unwrap() {
            sum += num;
            count += 1;
        }

        println!("average is {}", sum / count)
    });

    join!(send_task, avg_task);
})
```

`schedwalk` will then execute the future under all possible schedules. In this case there are just
two: one where `send_task` executes first and one where `avg_task` executes first. This will
reliably trigger the bug in our tests.

To make debugging easier, panics and deadlocks will print the polling schedule as a string to
standard error. Setting the environment variable `SCHEDULE` to this will execute only the exact
failing schedule. The above example will print `panic in SCHEDULE=01`. Executing the test again with
`SCHEDULE=01 cargo test example` will then execute only that exact schedule.

## Caveats

There are a few important caveats to `schedwalk`:
- `schedwalk` assumes determinism. Futures must spawn and poll futures in the same order every time.
  I.e. there can be no thread-local or global state influencing the order futures are polled and no
  external IO can influence the system.
- `schedwalk` will exhaustively walk all possible schedules. In cases of high amounts of
  futures that can be polled concurrently this can quickly become intractable.