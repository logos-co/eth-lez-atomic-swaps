cmake_minimum_required(VERSION 3.14)
project(SwapTests LANGUAGES CXX)

include(LogosTest)

logos_test(
    NAME swap_tests
    MODULE_SOURCES
        ../src/swap_impl.cpp
        ../src/swap_delivery_adapter.cpp
    TEST_SOURCES
        main.cpp
        test_swap.cpp
    EXTRA_INCLUDES
        ../lib
    MOCK_C_SOURCES
        mocks/mock_swap_ffi.cpp
)
