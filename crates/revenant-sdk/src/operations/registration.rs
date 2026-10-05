//! Typed executors deserialize before constructing a Send future. Every input handle is admitted through the task context before user work runs.
use super::OperationDescriptor;
use crate::{BoxFuture, Contract, Error, Handle, Result, TaskContext};
use serde_json::Value;
use std::{future::Future, sync::Arc};
/// Advanced native executor boundary. Prefer the operation macro for typed functions.
/// Registrations share executors across threads; returned futures must be Send.
/// The host owns the task execution guard, cancellation, and finalization.
pub trait OperationExecutor: Send + Sync + 'static {
    /// Implementations supplied without the typed constructor must validate and
    /// lease every input handle here, before operation work can begin. Validation
    /// must reject malformed input and propagate task cancellation or admission
    /// errors; captured leases survive release of the original resource owner.
    fn validate_input(&self, input: &Value, context: &TaskContext) -> Result<()>;
    /// Returns capabilities in the authoritative input contract, before execution.
    /// Native hosts use these identities to admit references owned by paged results.
    fn input_handles(&self, input: &Value) -> Result<Vec<Handle>>;
    /// Returns capabilities in the authoritative output contract for result ownership.
    /// Hosts retain output leases independently of short-lived execution contexts.
    fn output_handles(&self, output: &Value) -> Result<Vec<Handle>>;
    /// Executes validated input while borrowing this executor until future completion.
    /// Failures must retain a structured error code. Implementations checkpoint
    /// cooperatively and must not finalize the task guard owned by their host.
    fn execute(&self, input: Value, context: TaskContext) -> BoxFuture<'_, Result<Value>>;
}
/// A descriptor and executor generated together from the compiled Rust types.
/// Clones retain the shared executor. Registration rejects invalid identities or
/// schemas before the operation can be prepared for host execution.
#[derive(Clone)]
pub struct OperationRegistration {
    /// Compiled identity, documentation, and input/output contracts validated at registration.
    pub descriptor: OperationDescriptor,
    /// Shared native executor retained by registrations and prepared operations.
    /// Its validation step must capture every input capability before work starts.
    pub executor: Arc<dyn OperationExecutor>,
}

struct TypedExecutor<I, O, F> {
    function: F,
    marker: std::marker::PhantomData<fn(I) -> O>,
}
/// Send future returned by an operation; movable onto the native worker runtime.
pub trait OperationFuture<T>: Future<Output = Result<T>> + Send + 'static {}
impl<T, F: Future<Output = Result<T>> + Send + 'static> OperationFuture<T> for F {}

impl<I, O, F, Fut> OperationExecutor for TypedExecutor<I, O, F>
where
    I: Contract,
    O: Contract,
    F: Fn(I, TaskContext) -> Fut + Send + Sync + 'static,
    Fut: OperationFuture<O>,
{
    fn validate_input(&self, input: &Value, context: &TaskContext) -> Result<()> {
        let input: I = serde_json::from_value(input.clone())?;
        input.visit_handles(&mut |handle| context.lease(handle).map(|_| ()))
    }
    fn input_handles(&self, input: &Value) -> Result<Vec<Handle>> {
        let input: I = serde_json::from_value(input.clone())?;
        let mut handles = Vec::new();
        input.visit_handles(&mut |handle| {
            handles.push(handle.clone());
            Ok(())
        })?;
        Ok(handles)
    }
    fn output_handles(&self, output: &Value) -> Result<Vec<Handle>> {
        let output: O = serde_json::from_value(output.clone())?;
        let mut handles = Vec::new();
        output.visit_handles(&mut |handle| {
            handles.push(handle.clone());
            Ok(())
        })?;
        Ok(handles)
    }
    /// Executes validated input while borrowing this executor until future completion.
    /// Failures must retain a structured error code. Implementations checkpoint
    /// cooperatively and must not finalize the task guard owned by their host.
    fn execute(&self, input: Value, context: TaskContext) -> BoxFuture<'_, Result<Value>> {
        // Deserialize before creating the future so no non-Send input is carried
        // across an await except where the user's native future permits it.
        let input = serde_json::from_value::<I>(input);
        match input {
            Err(error) => Box::pin(async move { Err(error.into()) }),
            Ok(input) => {
                let future = (self.function)(input, context);
                Box::pin(async move { Ok(serde_json::to_value(future.await?)?) })
            }
        }
    }
}
impl OperationRegistration {
    /// Constructs a typed executor with compiled input and output contracts.
    /// Rejects an empty or whitespace-containing ID and propagates contract
    /// generation failures such as asymmetric serialization or lossy integers.
    /// This constructor does not publish the operation or acquire native resources.
    pub fn new<I, O, F, Fut>(
        id: impl Into<String>,
        description: impl Into<String>,
        function: F,
    ) -> Result<Self>
    where
        I: Contract,
        O: Contract,
        F: Fn(I, TaskContext) -> Fut + Send + Sync + 'static,
        Fut: OperationFuture<O>,
    {
        let id = id.into();
        if id.is_empty() || id.chars().any(char::is_whitespace) {
            return Err(Error::new(
                "invalid_operation_id",
                "Operation id is empty or contains whitespace",
            ));
        }
        Ok(Self {
            descriptor: OperationDescriptor {
                id,
                description: description.into(),
                input: I::definition()?,
                output: O::definition()?,
                source: None,
            },
            executor: Arc::new(TypedExecutor::<I, O, F> {
                function,
                marker: std::marker::PhantomData,
            }),
        })
    }
}
