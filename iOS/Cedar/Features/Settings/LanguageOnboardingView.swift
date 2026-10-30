import SwiftUI

/// What a new install shows first, before connecting to a server: the language to use. It starts
/// on the device's language when the app has it, and its own words follow the choice.
struct LanguageOnboardingView: View {
    let done: (String) -> Void
    @State private var selection = AppLanguage.suggested(for: Locale.preferredLanguages).tag

    var body: some View {
        VStack(spacing: 0) {
            VStack(spacing: 10) {
                CedarBadge()
                    .padding(.bottom, 10)
                Text("Choose your language")
                    .font(.title2.weight(.bold))
                    .accessibilityAddTraits(.isHeader)
                // The same in the other languages, for whoever can't read the first line yet.
                Text(verbatim: "Sprache wählen · Choisissez votre langue · Elige tu idioma")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            .multilineTextAlignment(.center)
            .padding(.horizontal)
            .padding(.top, 48)
            .padding(.bottom, 8)

            List {
                LanguageList(selection: $selection)
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)

            Button {
                done(selection)
            } label: {
                Text("Continue")
            }
            .buttonStyle(ProminentButtonStyle())
            .padding()
        }
        .frame(maxWidth: 620)
        .frame(maxWidth: .infinity)
        .background(Theme.canvas)
        .environment(\.locale, Locale(identifier: selection))
    }
}

/// The offered languages, each in its own words, with a check on the one picked.
private struct LanguageList: View {
    @Binding var selection: String

    var body: some View {
        ForEach(AppLanguage.all) { language in
            let isSelected = language.tag == selection
            Button {
                selection = language.tag
            } label: {
                HStack {
                    Text(verbatim: language.name)
                        .foregroundStyle(Color.primary)
                    Spacer()
                    if isSelected {
                        Image(systemName: "checkmark")
                            .fontWeight(.semibold)
                            .foregroundStyle(Color.accentColor)
                    }
                }
                .contentShape(Rectangle())
            }
            .listRowBackground(Theme.surface)
            .accessibilityAddTraits(isSelected ? .isSelected : [])
        }
    }
}
