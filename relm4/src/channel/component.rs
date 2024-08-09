// Copyright 2022 System76 <info@system76.com>
// SPDX-License-Identifier: MIT or Apache-2.0

//! Contains various flavors of channels to send messages between components and workers.

use std::fmt::Debug;
use std::future::Future;
use std::sync::Arc;

use crate::component::AsyncComponent;
use crate::factory::{AsyncFactoryComponent, FactoryComponent};
use crate::{Component, Sender, ShutdownReceiver};

// Contains senders used by components and factories internally.
#[derive(Debug)]
struct ComponentSenderInner<Input, Output, CommandOutput>
where
    Input: Debug,
    CommandOutput: Send + 'static,
{
    /// Emits component inputs.
    input: Sender<Input>,
    /// Emits component outputs.
    output: Sender<Output>,
    /// Emits command outputs.
    command: Sender<CommandOutput>,
    shutdown: ShutdownReceiver,
}

impl<Input, Output, CommandOutput> ComponentSenderInner<Input, Output, CommandOutput>
where
    Input: Debug,
    CommandOutput: Send + 'static,
{
    /// Retrieve the sender for input messages.
    ///
    /// Useful to forward inputs from another component. If you just need to send input messages,
    /// [`input()`][Self::input] is more concise.
    #[must_use]
    fn input_sender(&self) -> &Sender<Input> {
        &self.input
    }

    /// Retrieve the sender for output messages.
    ///
    /// Useful to forward outputs from another component. If you just need to send output messages,
    /// [`output()`][Self::output] is more concise.
    #[must_use]
    fn output_sender(&self) -> &Sender<Output> {
        &self.output
    }

    /// Retrieve the sender for command output messages.
    ///
    /// Useful to forward outputs from another component. If you just need to send output messages,
    /// [`command()`][Self::command] is more concise.
    #[must_use]
    fn command_sender(&self) -> &Sender<CommandOutput> {
        &self.command
    }

    /// Emit an input to the component.
    fn input(&self, message: Input) {
        // Input messages should always be safe to send
        // because the runtime keeps the receiver alive.
        self.input.send(message).expect("The runtime of the component was shutdown. Maybe you accidentally dropped a controller?");
    }

    /// This is not public because factories can unwrap the result
    /// because they keep the output receiver alive internally.
    fn output(&self, message: Output) -> Result<(), Output> {
        self.output.send(message)
    }

    /// Spawns an asynchronous command.
    /// You can bind the the command to the lifetime of the component
    /// by using a [`ShutdownReceiver`].
    fn command<Cmd, Fut>(&self, cmd: Cmd)
    where
        Cmd: FnOnce(Sender<CommandOutput>, ShutdownReceiver) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let recipient = self.shutdown.clone();
        let sender = self.command.clone();
        crate::spawn(async move {
            cmd(sender, recipient).await;
        });
    }

    /// Spawns a synchronous command.
    ///
    /// This is particularly useful for CPU-intensive background jobs that
    /// need to run on a thread-pool in the background.
    ///
    /// If you expect the component to be dropped while
    /// the command is running take care while sending messages!
    fn spawn_command<Cmd>(&self, cmd: Cmd)
    where
        Cmd: FnOnce(Sender<CommandOutput>) + Send + 'static,
    {
        let sender = self.command.clone();
        crate::spawn_blocking(move || cmd(sender));
    }

    /// Spawns a future that will be dropped as soon as the factory component is shut down.
    ///
    /// Essentially, this is a simpler version of [`Self::command()`].
    fn oneshot_command<Fut>(&self, future: Fut)
    where
        Fut: Future<Output = CommandOutput> + Send + 'static,
    {
        // Async closures would be awesome here...
        self.command(move |out, shutdown| {
            shutdown
                .register(async move { out.send(future.await) })
                .drop_on_shutdown()
        });
    }

    /// Spawns a synchronous command.
    ///
    /// Essentially, this is a simpler version of [`Self::spawn_command()`].
    fn spawn_oneshot_command<Cmd>(&self, cmd: Cmd)
    where
        Cmd: FnOnce() -> CommandOutput + Send + 'static,
    {
        let handle = crate::spawn_blocking(cmd);
        self.oneshot_command(async move { handle.await.unwrap() })
    }
}

macro_rules! sender_impl {
    ($name:ident, $trait:ident) => {
        /// Contains senders to send and receive messages from a [`Component`].
        #[derive(Debug)]
        pub struct $name<C: $trait> {
            shared: Arc<ComponentSenderInner<C::Input, C::Output, C::CommandOutput>>,
        }

        impl<C: $trait> $name<C> {
            pub(crate) fn new(
                input: Sender<C::Input>,
                output: Sender<C::Output>,
                command: Sender<C::CommandOutput>,
                shutdown: ShutdownReceiver,
            ) -> Self {
                Self {
                    shared: Arc::new(ComponentSenderInner {
                        input,
                        output,
                        command,
                        shutdown,
                    }),
                }
            }

            /// Retrieve the sender for input messages.
            ///
            /// Useful to forward inputs from another component. If you just need to send input messages,
            /// [`input()`][Self::input] is more concise.
            #[must_use]
            pub fn input_sender(&self) -> &Sender<C::Input> {
                self.shared.input_sender()
            }

            /// Retrieve the sender for output messages.
            ///
            /// Useful to forward outputs from another component. If you just need to send output messages,
            /// [`output()`][Self::output] is more concise.
            #[must_use]
            pub fn output_sender(&self) -> &Sender<C::Output> {
                self.shared.output_sender()
            }

            /// Retrieve the sender for command output messages.
            ///
            /// Useful to forward outputs from another component. If you just need to send output messages,
            /// [`command()`][Self::command] is more concise.
            #[must_use]
            pub fn command_sender(&self) -> &Sender<C::CommandOutput> {
                self.shared.command_sender()
            }

            /// Emit an input to the component.
            pub fn input(&self, message: C::Input) {
                self.shared.input(message);
            }

            /// Emit an output to the component.
            ///
            /// Returns [`Err`] if all receivers were dropped,
            /// for example by [`detach`].
            ///
            /// [`detach`]: crate::component::Connector::detach
            pub fn output(&self, message: C::Output) -> Result<(), C::Output> {
                self.shared.output(message)
            }

            /// Spawns an asynchronous command.
            /// You can bind the the command to the lifetime of the component
            /// by using a [`ShutdownReceiver`].
            pub fn command<Cmd, Fut>(&self, cmd: Cmd)
            where
                Cmd: FnOnce(Sender<C::CommandOutput>, ShutdownReceiver) -> Fut + Send + 'static,
                Fut: Future<Output = ()> + Send,
            {
                self.shared.command(cmd)
            }

            /// Spawns a synchronous command.
            ///
            /// This is particularly useful for CPU-intensive background jobs that
            /// need to run on a thread-pool in the background.
            ///
            /// If you expect the component to be dropped while
            /// the command is running take care while sending messages!
            pub fn spawn_command<Cmd>(&self, cmd: Cmd)
            where
                Cmd: FnOnce(Sender<C::CommandOutput>) + Send + 'static,
            {
                self.shared.spawn_command(cmd)
            }

            /// Spawns a future that will be dropped as soon as the factory component is shut down.
            ///
            /// Essentially, this is a simpler version of [`Self::command()`].
            pub fn oneshot_command<Fut>(&self, future: Fut)
            where
                Fut: Future<Output = C::CommandOutput> + Send + 'static,
            {
                self.shared.oneshot_command(future)
            }

            /// Spawns a synchronous command that will be dropped as soon as the factory component is shut down.
            ///
            /// Essentially, this is a simpler version of [`Self::spawn_command()`].
            pub fn spawn_oneshot_command<Cmd>(&self, cmd: Cmd)
            where
                Cmd: FnOnce() -> C::CommandOutput + Send + 'static,
            {
                self.shared.spawn_oneshot_command(cmd)
            }
        }

        impl<C: $trait> Clone for $name<C> {
            fn clone(&self) -> Self {
                Self {
                    shared: Arc::clone(&self.shared),
                }
            }
        }
    };
}

sender_impl!(ComponentSender, Component);
sender_impl!(AsyncComponentSender, AsyncComponent);
sender_impl!(FactorySender, FactoryComponent);
sender_impl!(AsyncFactorySender, AsyncFactoryComponent);
