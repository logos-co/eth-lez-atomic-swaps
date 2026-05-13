#pragma once

#include <string>

void swapDeliverySetRuntimeLogosAPI(void* api);

std::string swapDeliveryMessagingInit(const std::string& configJson);
std::string swapDeliveryMessagingShutdown();
std::string swapDeliveryMessagingStatus();
std::string swapDeliveryPublishOffer(const std::string& configJson);
std::string swapDeliveryFetchOffers();
