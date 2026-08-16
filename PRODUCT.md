# Jcode Desktop: Product Vision

## Purpose

Jcode Desktop is the native desktop version of Jcode. It should expose Jcode's agent and session capabilities through a visual, spatial interface rather than reproducing the terminal UI inside a window.

The application will be built on the Jcode SDK. The SDK is responsible for connecting to the Jcode runtime, creating and resuming sessions, sending prompts, and streaming session events. The desktop application is responsible for workspace organization, interaction, rendering, and native platform behavior.

## Defining Experience

The defining interface is a **canvas of Jcode sessions**.

Each session appears as a **panel** containing the chat transcript and controls for that session. A user can have many panels open and move between them spatially. The arrangement should make it easy to understand where sessions are, preserve mental context, and return to ongoing work.

The interaction and visual language should be distinctive rather than resembling a conventional chat application or IDE. Its primary inspiration is the [Niri](https://github.com/YaLTeR/niri) compositor:

- Sessions occupy stable spatial positions.
- Navigation between panels should feel direct, fluid, and predictable.
- Movement and focus transitions should preserve spatial context.
- A map or overview should make the larger collection of panels understandable.
- Keyboard-first navigation should be excellent without compromising pointer and trackpad use.

The exact canvas model remains an open design decision. It may be a structured, Niri-like strip of panels with a zoomed-out overview, a freely arranged two-dimensional canvas, or a deliberate hybrid of the two.

## Session Panels

A panel represents one Jcode session. It should eventually support:

- A live, streaming chat transcript
- Prompt composition and submission
- Markdown formatting
- Syntax-highlighted code blocks
- LaTeX and mathematical notation
- Tool calls, progress, todos, and other structured Jcode events
- Text selection, links, copying, and transcript navigation
- Clear running, waiting, completed, disconnected, and failed states

Panels should remain responsive when transcripts are long and when several sessions are active simultaneously. Off-screen content and panels should not impose unnecessary rendering or layout work.

## Performance

High performance is a core product requirement, not a later optimization.

The application should:

- Be implemented in Rust.
- Use GPU acceleration where it materially improves rendering and animation.
- Keep input, scrolling, panel navigation, and animations responsive under load.
- Virtualize large transcripts and off-screen panels.
- Avoid recomputing or redrawing unchanged content.
- Incrementally lay out streaming responses instead of repeatedly rebuilding entire transcripts.
- Establish measurable frame-time, latency, memory, and startup targets as the implementation matures.

Much of the visible UI will be custom. The chosen UI architecture must permit Jcode-specific panels, transitions, overview behavior, transcript presentation, and interaction without forcing the application into a conventional component-library appearance.

## Platform Requirements

Development will happen primarily on Linux, but macOS is a first-class target.

Cross-platform behavior should be shared where practical, while platform-specific integration should be native where it matters. On macOS this includes areas such as:

- Window lifecycle and application menus
- Retina scaling
- Keyboard conventions and shortcuts
- Trackpad behavior
- Clipboard and drag-and-drop integration
- Text input and IME behavior
- Notifications and other operating-system services
- Accessibility

The architecture should not treat macOS as a Linux build that merely happens to compile.

## Self-Development

Jcode's self-development workflow must extend to the desktop application.

The developer should be able to rebuild and update the running desktop application **without closing its window**. Ideally, the long-lived host retains the native window and expensive platform or GPU resources while reloadable application code is replaced.

A successful reload should preserve as much useful state as safely possible, including:

- Open session panels
- Panel arrangement and focused panel
- Scroll positions
- Draft input
- Connection to the Jcode runtime, when compatible

Reload failures should leave the current application usable and display an actionable error rather than terminating the process. State and reload boundaries must be designed deliberately so that rapid self-development remains reliable as the application grows.

## Architectural Principles

1. **Build on the Jcode SDK.** Do not duplicate the runtime or communicate through private internal interfaces when the SDK can provide the capability.
2. **Keep product state separate from rendering.** Sessions, panel placement, drafts, and navigation state should survive renderer changes and development reloads.
3. **Design around the spatial workspace.** The panel canvas is the primary product model, not an effect added to a conventional chat layout.
4. **Prefer incremental work.** Streaming, layout, rendering, and event processing should update only affected regions.
5. **Preserve native behavior.** Custom visuals must not come at the expense of correct text input, accessibility, clipboard behavior, or platform conventions.
6. **Measure performance.** Important interaction paths should have benchmarks or instrumentation rather than relying only on subjective smoothness.
7. **Keep framework decisions reversible where reasonable.** The document and session model should not be inseparable from a particular renderer.

## UI Technology Decision

The Rust UI stack has not yet been selected.

The principal options currently under consideration are:

- **GPUI**, which provides an integrated component, layout, input, focus, text, window, and rendering framework while still allowing a custom visual language.
- **A custom stack based on winit, wgpu, Vello, and Parley**, which provides maximum control but requires Jcode to own considerably more UI infrastructure.

The decision should be made through a focused prototype that tests the defining risks:

- A large spatial canvas with many virtualized panels
- Smooth navigation, zoom, and overview transitions
- A streaming, selectable, richly formatted transcript
- Correct text editing and IME behavior
- Linux and macOS platform behavior
- Reloading application code without closing the host window

The framework should be selected based on measured capability and engineering cost, not visual assumptions. A unique interface can be built with either approach.

## Initial Product Boundary

The first meaningful version should prove the central workflow:

1. Start the desktop application and connect through the Jcode SDK.
2. Create or resume multiple Jcode sessions.
3. Display each session as a panel with a live transcript.
4. Navigate fluidly between panels and enter an overview of the workspace.
5. Send prompts and observe streaming updates.
6. Rebuild and reload desktop application code without closing the window or losing the workspace.
7. Run the same core application on Linux and macOS with appropriate native behavior.

Features outside this workflow should follow after the panel model, transcript rendering, SDK integration, performance, and reload architecture are sound.
