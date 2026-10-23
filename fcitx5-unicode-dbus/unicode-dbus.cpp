#include <fcitx-config/configuration.h>
#include <fcitx-config/rawconfig.h>
#include <fcitx-utils/dbus/bus.h>
#include <fcitx-utils/dbus/objectvtable.h>
#include <fcitx-utils/key.h>
#include <fcitx-utils/log.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/event.h>
#include <fcitx/instance.h>
#include <fcitx/inputcontext.h>
#include <dbus_public.h>
#include <unicode_public.h>
#include <stdexcept>

namespace fcitx {

class UnicodeDBus : public AddonInstance {
public:
    explicit UnicodeDBus(Instance *instance);

    void triggerUnicode();
    void triggerUnicodeHex();

private:
    Key directUnicodeKey();

    class DBusObject : public dbus::ObjectVTable<DBusObject> {
    public:
        UnicodeDBus *parent_ = nullptr;

        void triggerUnicode() { parent_->triggerUnicode(); }
        void triggerUnicodeHex() { parent_->triggerUnicodeHex(); }

        FCITX_OBJECT_VTABLE_METHOD(triggerUnicode, "TriggerUnicode", "", "");
        FCITX_OBJECT_VTABLE_METHOD(triggerUnicodeHex, "TriggerUnicodeHex", "", "");
    } object_;

    FCITX_ADDON_DEPENDENCY_LOADER(dbus, instance_->addonManager());
    FCITX_ADDON_DEPENDENCY_LOADER(unicode, instance_->addonManager());
    Instance *instance_;
};

UnicodeDBus::UnicodeDBus(Instance *instance) : instance_(instance) {
    object_.parent_ = this;
    if (!dbus()) {
        throw std::runtime_error("dbus module is not available");
    }
    auto *bus = dbus()->call<IDBusModule::bus>();
    if (!bus || !bus->addObjectVTable("/unicode", "org.fcitx.Fcitx.Unicode1", object_)) {
        throw std::runtime_error("failed to register D-Bus object");
    }
}

void UnicodeDBus::triggerUnicode() {
    auto *unicode = this->unicode();
    if (!unicode) {
        FCITX_WARN() << "unicode addon is not loaded";
        return;
    }
    auto *ic = instance_->mostRecentInputContext();
    if (!ic) {
        return;
    }
    if (!unicode->call<IUnicode::trigger>(ic)) {
        FCITX_WARN() << "failed to trigger unicode input";
    }
}

void UnicodeDBus::triggerUnicodeHex() {
    auto *ic = instance_->mostRecentInputContext();
    if (!ic) {
        return;
    }
    KeyEvent event(ic, directUnicodeKey());
    ic->keyEvent(event);
}

Key UnicodeDBus::directUnicodeKey() {
    if (auto *unicode = this->unicode()) {
        if (const auto *config = unicode->getConfig()) {
            RawConfig raw;
            config->save(raw);
            if (const auto *value = raw.valueByPath("DirectUnicodeMode/0")) {
                auto keys = Key::keyListFromString(*value);
                if (!keys.empty()) {
                    return keys.front();
                }
            }
        }
    }
    return Key("Control+Shift+U");
}

class UnicodeDBusFactory : public AddonFactory {
    AddonInstance *create(AddonManager *manager) override {
        return new UnicodeDBus(manager->instance());
    }
};

} // namespace fcitx

FCITX_ADDON_FACTORY(fcitx::UnicodeDBusFactory);
