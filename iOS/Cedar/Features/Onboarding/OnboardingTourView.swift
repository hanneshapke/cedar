import SwiftUI

/// The feature tour a new install sees once, after picking a language: seven pages, each a
/// picture of the app doing the thing, a title and two lines. Swipe or Continue; Skip ends it
/// early. Settings → About shows it again.
struct OnboardingTourView: View {
    let done: () -> Void
    @State private var page = 0
    private let pages = TourPage.all

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                CedarWordmark(size: 18)
                Spacer()
                Button("Skip") { done() }
                    .font(.subheadline.weight(.medium))
                    .opacity(isLast ? 0 : 1)
                    .disabled(isLast)
            }
            .padding(.horizontal, 20)
            .padding(.top, 12)

            TabView(selection: $page) {
                ForEach(Array(pages.enumerated()), id: \.offset) { index, item in
                    TourPageView(page: item, active: page == index)
                        .tag(index)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: .never))

            PageDots(count: pages.count, index: page)
                .padding(.bottom, 16)

            Button {
                if isLast {
                    done()
                } else {
                    withAnimation(.snappy(duration: 0.35)) { page += 1 }
                }
            } label: {
                Text(isLast ? "Get Started" : "Continue")
            }
            .buttonStyle(ProminentButtonStyle())
            .padding(.horizontal)
            .padding(.bottom)
        }
        .frame(maxWidth: 620)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    private var isLast: Bool {
        page == pages.count - 1
    }
}

/// One page: the picture above, the words below.
private struct TourPageView: View {
    let page: TourPage
    /// Whether this is the page on screen; the picture builds up when it becomes so.
    let active: Bool
    @State private var shown = false
    /// Read when the page appears and again on coming back, from the App Store after installing it.
    @State private var tailscaleInstalled = true
    @Environment(\.openURL) private var openURL
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        VStack(spacing: 0) {
            page.illustration.view(shown: shown)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding(.horizontal, 12)
            VStack(spacing: 10) {
                Text(page.title)
                    .font(.title2.weight(.bold))
                    .accessibilityAddTraits(.isHeader)
                Text(page.text)
                    .font(.body)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .multilineTextAlignment(.center)
            .frame(maxWidth: 420)
            .frame(minHeight: 120, alignment: .top)
            .padding(.horizontal, 28)
            .padding(.vertical, 12)
            .accessibilityElement(children: .combine)
            if page.offersTailscale && !tailscaleInstalled {
                getTailscale
            }
        }
        .onAppear { tailscaleInstalled = TailscaleApp.isInstalled }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { tailscaleInstalled = TailscaleApp.isInstalled }
        }
        .onChange(of: active, initial: true) { _, active in
            // The page beside the one on screen is built ahead of time; its picture waits.
            shown = active
        }
    }
}

private extension TourPageView {
    /// On the page about connecting, when the Tailscale app isn't on this device: the phone needs
    /// it to reach a server shared over Tailscale, so the tour offers it before Settings does.
    var getTailscale: some View {
        VStack(spacing: 8) {
            Text("Cedar usually reaches your server through Tailscale. Install the Tailscale app on this device and log in to the same tailnet as your server.")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            Button {
                openURL(TailscaleApp.appStoreURL)
            } label: {
                Label("Get Tailscale", systemImage: "arrow.down.app")
                    .font(.subheadline.weight(.semibold))
            }
            .buttonStyle(.bordered)
        }
        .frame(maxWidth: 420)
        .padding(.horizontal, 28)
        .padding(.bottom, 12)
    }
}

/// The dots under the pages: the current one long and green.
private struct PageDots: View {
    let count: Int
    let index: Int

    var body: some View {
        HStack(spacing: 6) {
            ForEach(0..<count, id: \.self) { i in
                Capsule()
                    .fill(i == index ? Theme.green : Theme.line)
                    .frame(width: i == index ? 22 : 7, height: 7)
                    .animation(.snappy(duration: 0.3), value: index)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: L10n.format("Page %lld of %lld", index + 1, count)))
    }
}

/// A page's words and picture.
struct TourPage {
    let title: LocalizedStringKey
    let text: LocalizedStringKey
    let illustration: TourIllustration
    /// Whether the page offers the Tailscale app when it isn't installed.
    var offersTailscale = false

    static let all: [TourPage] = [
        TourPage(title: "Your agent, on your own server",
                 text: "Cedar runs where you put it. This app talks to it; your keys, files and conversations never leave your server.",
                 illustration: .server),
        TourPage(title: "Watch it work",
                 text: "Replies stream in as Cedar reasons, searches the web, runs code in its sandbox and reads your mail and files. Every step is there to open.",
                 illustration: .work),
        TourPage(title: "Answers as cards",
                 text: "Weather, charts, recipes, routes, flights, stocks and products come as native cards you can tap, not walls of text.",
                 illustration: .cards),
        TourPage(title: "You stay in charge",
                 text: "For bigger requests Cedar lays out a plan and waits for your go-ahead, and asks when something is unclear.",
                 illustration: .plans),
        TourPage(title: "Agents that run on a schedule",
                 text: "Turn a conversation into an agent that runs on its own. A push tells you when it has something, and the result is here.",
                 illustration: .schedules),
        TourPage(title: "It remembers, privately",
                 text: "Your name, preferences and projects carry across conversations. See everything Cedar remembers and delete any of it.",
                 illustration: .memory),
        TourPage(title: "Connect your server",
                 text: "Scan the QR code from the Cedar Mac app or the stack script, or type the address and token. That is the whole setup.",
                 illustration: .connect,
                 offersTailscale: true),
    ]
}
